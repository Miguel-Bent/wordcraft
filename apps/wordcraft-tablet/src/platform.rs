//! Android services for WordCraft: documents picked and saved through the system file picker.
//!
//! Like the browser build, documents travel as bytes. Opening hands the picked file's bytes to the
//! app through the inbox; saving hands the bytes to `WordCraftActivity`, which writes them where the
//! user chose. Android gives content URIs, not paths, so the app never needs a path.
//!
//! `WordCraftActivity` calls the `native*` functions below on its own threads. They only queue
//! events; the UI thread applies them each frame ([`Platform::apply_events`]), because the save
//! tickets are shared with the UI and are not `Send`.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::mem::ManuallyDrop;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use jni::objects::{JByteArray, JClass, JObject, JString, JValue};
use jni::sys::{jbyteArray, jint};
use jni::{JNIEnv, JavaVM};
use winit::platform::android::activity::AndroidApp;
use wordcraft_ui_egui::async_saves::{AsyncSaves, SaveOutcome};
use wordcraft_ui_egui::{Inbox, Services};

/// What the activity reported, waiting for the UI thread.
enum Event {
    Picked { name: String, bytes: Vec<u8> },
    Saved { id: i32, outcome: SaveOutcome },
}

/// Events from the activity's threads, applied on the UI thread.
static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());
/// Bytes a save staged, keyed by request id, until the activity takes them.
static PENDING_SAVES: Mutex<BTreeMap<i32, Vec<u8>>> = Mutex::new(BTreeMap::new());
static NEXT_REQUEST: AtomicI32 = AtomicI32::new(1);
/// The activity handle, set once at start-up, for calling back into Java.
static ANDROID: OnceLock<AndroidApp> = OnceLock::new();
/// Wakes the UI when an event arrives from another thread.
static REPAINT: Mutex<Option<egui::Context>> = Mutex::new(None);

/// Save status codes, as `WordCraftActivity` sends them.
const STATUS_SAVED: i32 = 0;
const STATUS_CANCELLED: i32 = 1;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn next_request() -> i32 {
    NEXT_REQUEST.fetch_add(1, Ordering::Relaxed)
}

fn queue(event: Event) {
    lock(&EVENTS).push(event);
    if let Some(ctx) = lock(&REPAINT).as_ref() {
        ctx.request_repaint();
    }
}

/// Stores the activity handle for [`Platform`]. Called once, before the app starts.
pub fn set_activity(app: AndroidApp) {
    // A second call would be a start-up bug; the first handle stays in use.
    let _ = ANDROID.set(app);
}

/// The file picker's MIME types for an open purpose (`document`, `picture`, `font`, …).
fn mime_types(purpose: &str) -> Vec<String> {
    match purpose {
        "picture" => vec!["image/*".to_string()],
        _ => Vec::new(),
    }
}

/// The MIME type a saved file gets, from its extension (the picker uses it to keep the name).
fn mime_for(name: &str) -> &'static str {
    match Path::new(name).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        Some("odt") => "application/vnd.oasis.opendocument.text",
        Some("rtf") => "application/rtf",
        Some("pdf") => "application/pdf",
        Some("md") => "text/markdown",
        Some("txt") => "text/plain",
        Some("html") | Some("htm") => "text/html",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    }
}

/// Runs `call` with a JNI environment and the activity object, on the calling thread.
fn with_activity<R>(call: impl FnOnce(&mut JNIEnv, &JObject) -> Result<R, String>) -> Result<R, String> {
    let app = ANDROID.get().ok_or("the Android activity is not ready")?;
    // SAFETY: the pointers come from the running activity; the VM outlives the app.
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }.map_err(|e| e.to_string())?;
    let mut guard = vm.attach_current_thread().map_err(|e| e.to_string())?;
    // SAFETY: the activity reference is owned by android-activity; never delete it here.
    let activity = ManuallyDrop::new(unsafe { JObject::from_raw(app.activity_as_ptr().cast()) });
    call(&mut guard, &activity)
}

/// The Services for Android, and the queue the UI drains each frame.
pub struct Platform {
    inbox: Inbox,
    saves: AsyncSaves,
    /// Save request id → the ticket the UI is waiting on.
    tickets: Rc<RefCell<BTreeMap<i32, u64>>>,
}

impl Platform {
    pub fn new(ctx: &egui::Context, inbox: Inbox) -> Self {
        *lock(&REPAINT) = Some(ctx.clone());
        Self { inbox, saves: AsyncSaves::default(), tickets: Rc::new(RefCell::new(BTreeMap::new())) }
    }

    /// The host hooks the app calls: opening and saving through the system picker.
    pub fn services(&self) -> Services {
        let saves = self.saves.clone();
        let tickets = self.tickets.clone();
        Services {
            inbox: Some(self.inbox.clone()),
            async_saves: Some(self.saves.clone()),
            open_async: Some(Box::new(|purpose: &str| {
                let request = next_request();
                let mimes = mime_types(purpose);
                let multiple = purpose == "font";
                let result = with_activity(|env, activity| {
                    let array = env.new_object_array(mimes.len() as i32, "java/lang/String", JObject::null()).map_err(|e| e.to_string())?;
                    for (index, mime) in mimes.iter().enumerate() {
                        let text = env.new_string(mime).map_err(|e| e.to_string())?;
                        env.set_object_array_element(&array, index as i32, text).map_err(|e| e.to_string())?;
                    }
                    env.call_method(
                        activity,
                        "openDocument",
                        "(I[Ljava/lang/String;Z)V",
                        &[JValue::Int(request), JValue::Object(&array), JValue::Bool(multiple as u8)],
                    )
                    .map(|_| ())
                    .map_err(|e| e.to_string())
                });
                if let Err(error) = result {
                    log::error!("cannot open the file picker: {error}");
                }
            })),
            // Exports ask for a name; the picker decides where the file goes.
            pick_save: Some(Box::new(|name: &str| Some(name.to_string()))),
            download: Some(Box::new(move |path: &str, bytes: &[u8]| {
                let name = Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "document".to_string());
                let request = next_request();
                lock(&PENDING_SAVES).insert(request, bytes.to_vec());
                tickets.borrow_mut().insert(request, saves.start());
                with_activity(|env, activity| {
                    let text = env.new_string(&name).map_err(|e| e.to_string())?;
                    let mime = env.new_string(mime_for(&name)).map_err(|e| e.to_string())?;
                    env.call_method(
                        activity,
                        "createDocument",
                        "(ILjava/lang/String;Ljava/lang/String;)V",
                        &[JValue::Int(request), JValue::Object(&text), JValue::Object(&mime)],
                    )
                    .map(|_| ())
                    .map_err(|e| e.to_string())
                })
                .inspect_err(|_| {
                    lock(&PENDING_SAVES).remove(&request);
                })
            })),
            clipboard_text: Some(Box::new(|| clipboard_string("clipboardText"))),
            clipboard_html: Some(Box::new(|| clipboard_string("clipboardHtml"))),
            clipboard_formats: Some(Box::new(|| {
                clipboard_string("clipboardMimeTypes")
                    .map(|types| types.lines().map(str::to_string).filter(|t| !t.is_empty()).collect())
                    .unwrap_or_default()
            })),
            set_clipboard_html: Some(Box::new(|html: &str, text: &str, _mathml: &str| set_clipboard(html, text).is_ok())),
            clipboard_picture: Some(Box::new(|| clipboard_image().map(wordcraft_ui_egui::paste_picture::encoded_picture))),
            print: Some(Box::new(|pdf: &[u8]| print_pdf(pdf))),
            ..Default::default()
        }
    }

    /// Applies what the activity reported since the last frame. Called on the UI thread.
    pub fn apply_events(&self) {
        let events = std::mem::take(&mut *lock(&EVENTS));
        for event in events {
            match event {
                Event::Picked { name, bytes } => lock(&self.inbox).push((name, bytes)),
                Event::Saved { id, outcome } => {
                    lock(&PENDING_SAVES).remove(&id);
                    if let Some(ticket) = self.tickets.borrow_mut().remove(&id) {
                        self.saves.finish(ticket, outcome);
                    }
                }
            }
        }
    }
}

/// Calls a `String clipboard…()` method of the activity; `None` when it returns null or fails.
fn clipboard_string(method: &str) -> Option<String> {
    with_activity(|env, activity| {
        let value = env.call_method(activity, method, "()Ljava/lang/String;", &[]).and_then(|v| v.l()).map_err(|e| e.to_string())?;
        if value.is_null() {
            return Ok(None);
        }
        let text: String = env.get_string(&JString::from(value)).map_err(|e| e.to_string())?.into();
        Ok(Some(text))
    })
    .ok()
    .flatten()
}

/// The plain text on the system clipboard, if any (for Ctrl+V).
pub fn clipboard_text() -> Option<String> {
    clipboard_string("clipboardText")
}

/// The picture on the system clipboard, as its encoded bytes (a copied image or picture file).
fn clipboard_image() -> Option<Vec<u8>> {
    with_activity(|env, activity| {
        let value = env.call_method(activity, "clipboardImage", "()[B", &[]).and_then(|v| v.l()).map_err(|e| e.to_string())?;
        if value.is_null() {
            return Ok(None);
        }
        let bytes = env.convert_byte_array(JByteArray::from(value)).map_err(|e| e.to_string())?;
        Ok(Some(bytes))
    })
    .ok()
    .flatten()
}

/// Opens the system print dialog with a PDF (WordCraft's print output).
fn print_pdf(pdf: &[u8]) -> Result<(), String> {
    with_activity(|env, activity| {
        let bytes = env.byte_array_from_slice(pdf).map_err(|e| e.to_string())?;
        let name = env.new_string("WordCraft").map_err(|e| e.to_string())?;
        env.call_method(activity, "printPdf", "([BLjava/lang/String;)V", &[JValue::Object(&bytes), JValue::Object(&name)])
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
}

/// Puts text (and its HTML, when given) on the system clipboard.
fn set_clipboard(html: &str, text: &str) -> Result<(), String> {
    with_activity(|env, activity| {
        let html_value = if html.is_empty() { JObject::null() } else { env.new_string(html).map_err(|e| e.to_string())?.into() };
        let text_value = env.new_string(text).map_err(|e| e.to_string())?;
        env.call_method(
            activity,
            "setClipboard",
            "(Ljava/lang/String;Ljava/lang/String;)Z",
            &[JValue::Object(&html_value), JValue::Object(&text_value)],
        )
        .and_then(|v| v.z())
        .map(|_| ())
        .map_err(|e| e.to_string())
    })
}

/// Copies plain text to the system clipboard. The app hands its copies to egui, which has no
/// clipboard on Android, so the app forwards them here.
pub fn copy_text(text: &str) {
    if let Err(error) = set_clipboard("", text) {
        log::error!("cannot copy to the clipboard: {error}");
    }
}

// ---- Called by WordCraftActivity (Java threads) -------------------------------------------------

/// A document the user picked: its name and bytes.
#[unsafe(no_mangle)]
pub extern "system" fn Java_wordcraft_android_WordCraftActivity_nativeOnPicked<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    _request: jint,
    name: JString<'local>,
    bytes: JByteArray<'local>,
) {
    let name: String = match env.get_string(&name) {
        Ok(text) => text.into(),
        Err(error) => {
            log::error!("picked file has no readable name: {error}");
            return;
        }
    };
    match env.convert_byte_array(&bytes) {
        Ok(bytes) => queue(Event::Picked { name, bytes }),
        Err(error) => log::error!("picked file can't be read: {error}"),
    }
}

/// The bytes a save staged, taken for writing (null when there are none).
#[unsafe(no_mangle)]
pub extern "system" fn Java_wordcraft_android_WordCraftActivity_nativeTakeSaveBytes<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    request: jint,
) -> jbyteArray {
    let bytes = lock(&PENDING_SAVES).get(&request).cloned();
    match bytes {
        Some(bytes) => env.byte_array_from_slice(&bytes).map(|a| a.into_raw()).unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}

/// How a save ended. Failures carry the reason.
#[unsafe(no_mangle)]
pub extern "system" fn Java_wordcraft_android_WordCraftActivity_nativeOnSaved<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    request: jint,
    status: jint,
    message: JString<'local>,
) {
    let outcome = match status {
        STATUS_SAVED => SaveOutcome::Saved,
        STATUS_CANCELLED => SaveOutcome::Cancelled,
        _ => {
            let reason: String = env.get_string(&message).map(Into::into).unwrap_or_default();
            SaveOutcome::Failed(reason)
        }
    };
    queue(Event::Saved { id: request, outcome });
}
