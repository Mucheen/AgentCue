use objc2::rc::{autoreleasepool, Retained};
use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
use objc2_application_services::{
    kAXTrustedCheckOptionPrompt, AXError, AXIsProcessTrusted, AXIsProcessTrustedWithOptions,
    AXUIElement,
};
use objc2_core_foundation::{
    kCFBooleanTrue, CFArray, CFBoolean, CFDictionary, CFRetained, CFString, CFType, ConcreteType,
    Type, CFURL,
};
use objc2_core_graphics::CGEvent;
use objc2_foundation::NSString;
use std::collections::VecDeque;
use std::process::Command;
use std::ptr::{self, NonNull};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::Url;
use uuid::Uuid;

const BUNDLE_ID: &str = "com.google.antigravity";
const RETURN_KEY: u16 = 0x24;
const SEARCH_TIMEOUT: Duration = Duration::from_secs(3);
static JUMP_LOCK: Mutex<()> = Mutex::new(());

pub fn open_conversation(raw_id: &str) -> Result<(), String> {
    let id = Uuid::parse_str(raw_id).map_err(|_| "Antigravity 会话 ID 无效")?;
    let _jump = JUMP_LOCK
        .try_lock()
        .map_err(|_| "正在打开另一条 Antigravity 会话")?;
    if !accessibility_is_allowed() {
        return Err("请在系统设置的「隐私与安全性 → 辅助功能」中允许 AgentCue，然后重试".into());
    }
    autoreleasepool(|_| navigate_to_conversation(id))
}

fn accessibility_is_allowed() -> bool {
    if unsafe { AXIsProcessTrusted() } {
        return true;
    }
    // Request the standard system prompt only when the user invokes a jump.
    if let Some(enabled) = unsafe { kCFBooleanTrue } {
        let prompt = unsafe { kAXTrustedCheckOptionPrompt };
        let options = CFDictionary::from_slices(&[prompt], &[enabled]);
        return unsafe { AXIsProcessTrustedWithOptions(Some(options.as_opaque())) };
    }
    false
}

fn running_antigravity() -> Option<Retained<NSRunningApplication>> {
    NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(BUNDLE_ID))
        .firstObject()
}

fn navigate_to_conversation(id: Uuid) -> Result<(), String> {
    let application = match running_antigravity() {
        Some(app) => app,
        None => {
            let launched = Command::new("open")
                .args(["-b", BUNDLE_ID])
                .status()
                .is_ok_and(|status| status.success());
            if !launched {
                return Err("未能启动 Antigravity 桌面版".into());
            }
            wait_for(running_antigravity).ok_or("Antigravity 尚未启动，请稍后重试")?
        }
    };

    let pid = application.processIdentifier();
    let app = unsafe { AXUIElement::new_application(pid) };
    let _ = unsafe { app.set_messaging_timeout(0.35) };
    #[allow(deprecated)]
    let _ =
        application.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);

    let (window, already_open) =
        wait_for(|| select_window(&app, id)).ok_or("Antigravity 窗口尚未就绪")?;
    let _ = perform(&window, "AXRaise");
    if already_open {
        return Ok(());
    }

    let link = find_element(&window, |element| {
        has_text(element, "AXRole", "AXLink") && element_conversation_url(element) == Some(id)
    });
    if let Some(link) = link {
        if perform(&link, "AXPress")
            && wait_for(|| (shown_conversation(&window) == Some(id)).then_some(())).is_some()
        {
            return Ok(());
        }
    }

    let input = match search_input(&window) {
        Some(input) => input,
        None => {
            let button = find_element(&window, |el| {
                has_text(el, "AXRole", "AXButton")
                    && ["AXTitle", "AXDescription", "AXHelp"]
                        .iter()
                        .any(|name| has_text(el, name, "Search Conversations"))
            })
            .ok_or("找不到 Antigravity 的会话搜索按钮")?;
            if !perform(&button, "AXPress") {
                return Err("未能打开 Antigravity 的会话搜索".into());
            }
            wait_for(|| search_input(&window)).ok_or("Antigravity 的会话搜索尚未就绪")?
        }
    };

    let query = id.to_string();
    let set = unsafe {
        input.set_attribute_value(&CFString::from_str("AXValue"), &CFString::from_str(&query))
    };
    if set != AXError::Success {
        return Err("未能在 Antigravity 中输入会话 ID".into());
    }

    let result = wait_for(|| selected_search_result(&window, &query))
        .ok_or("Antigravity 搜索中没有这条会话")?;

    if perform(&result, "AXPress")
        && wait_for(|| (shown_conversation(&window) == Some(id)).then_some(())).is_some()
    {
        return Ok(());
    }

    // Chromium may expose the selected result without an AXPress action.
    // A Return event is sent only to Antigravity with the matching search open.
    let focused = attribute::<AXUIElement>(&app, "AXFocusedWindow")
        .filter(|window| selected_search_result(window, &query).is_some())
        .ok_or("Antigravity 的搜索选择已变化，请重试")?;
    for down in [true, false] {
        let event = CGEvent::new_keyboard_event(None, RETURN_KEY, down)
            .ok_or("未能确认 Antigravity 的搜索结果")?;
        CGEvent::post_to_pid(pid, Some(&event));
    }

    wait_for(|| (shown_conversation(&focused) == Some(id)).then_some(()))
        .ok_or_else(|| "Antigravity 未打开匹配的会话".into())
}

fn selected_search_result(window: &AXUIElement, query: &str) -> Option<CFRetained<AXUIElement>> {
    let input = search_input(window)?;
    if !has_text(&input, "AXValue", query) {
        return None;
    }
    find_element(window, |element| is_selected_result(element, query))
}

fn select_window(app: &AXUIElement, id: Uuid) -> Option<(CFRetained<AXUIElement>, bool)> {
    let windows = elements(app, "AXWindows");
    if let Some(window) = windows
        .iter()
        .find(|window| shown_conversation(window) == Some(id))
    {
        return Some((window.clone(), true));
    }
    attribute(app, "AXFocusedWindow")
        .or_else(|| windows.into_iter().next())
        .map(|window| (window, false))
}

fn shown_conversation(window: &AXUIElement) -> Option<Uuid> {
    let mut shown = None;
    find_element(window, |el| {
        if has_text(el, "AXRole", "AXWebArea") {
            shown = element_conversation_url(el);
        }
        shown.is_some()
    })?;
    shown
}

fn element_conversation_url(element: &AXUIElement) -> Option<Uuid> {
    let url = raw_attribute(element, "AXURL")?;
    let value = match url.downcast::<CFURL>() {
        Ok(url) => url.string().to_string(),
        Err(value) => value.downcast::<CFString>().ok()?.to_string(),
    };
    conversation_id_from_url(&value)
}

fn conversation_id_from_url(value: &str) -> Option<Uuid> {
    let url = Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
    {
        return None;
    }
    Uuid::parse_str(url.path().strip_prefix("/c/")?).ok()
}

fn search_input(window: &AXUIElement) -> Option<CFRetained<AXUIElement>> {
    find_element(window, |el| {
        has_text(el, "AXRole", "AXComboBox")
            && has_text(el, "AXPlaceholderValue", "Search for conversations...")
    })
}

fn is_selected_result(element: &AXUIElement, query: &str) -> bool {
    attribute::<CFBoolean>(element, "AXSelected").is_some_and(|v| v.value())
        && (["AXValue", "AXTitle", "AXDescription"].iter().any(|name| {
            attribute::<CFString>(element, name).is_some_and(|text| {
                text.to_string()
                    .split_whitespace()
                    .any(|word| word == query)
            })
        }) || elements(element, "AXChildren")
            .iter()
            .any(|child| has_text(child, "AXValue", query) || has_text(child, "AXTitle", query)))
}

fn raw_attribute(element: &AXUIElement, name: &str) -> Option<CFRetained<CFType>> {
    let mut value = ptr::null();
    let status = unsafe {
        element.copy_attribute_value(&CFString::from_str(name), NonNull::from(&mut value))
    };
    (status == AXError::Success)
        .then(|| NonNull::new(value.cast_mut()))
        .flatten()
        .map(|value| unsafe { CFRetained::from_raw(value) })
}

fn attribute<T: ConcreteType>(element: &AXUIElement, name: &str) -> Option<CFRetained<T>> {
    raw_attribute(element, name)?.downcast().ok()
}

fn has_text(element: &AXUIElement, name: &str, value: &str) -> bool {
    attribute::<CFString>(element, name).is_some_and(|text| text.to_string() == value)
}

fn elements(element: &AXUIElement, name: &str) -> Vec<CFRetained<AXUIElement>> {
    let Some(children) = attribute::<CFArray>(element, name) else {
        return Vec::new();
    };
    // AXChildren and AXWindows contain references to Core Foundation objects.
    let children = unsafe { CFRetained::cast_unchecked::<CFArray<CFType>>(children) };
    children
        .iter()
        .filter_map(|child| child.downcast().ok())
        .collect()
}

fn perform(element: &AXUIElement, action: &str) -> bool {
    (unsafe { element.perform_action(&CFString::from_str(action)) }) == AXError::Success
}

fn find_element(
    root: &AXUIElement,
    mut matches: impl FnMut(&AXUIElement) -> bool,
) -> Option<CFRetained<AXUIElement>> {
    let deadline = Instant::now() + Duration::from_millis(900);
    let mut queue = VecDeque::from([root.retain()]);
    for _ in 0..600 {
        if Instant::now() >= deadline {
            break;
        }
        let element = queue.pop_front()?;
        if matches(&element) {
            return Some(element);
        }
        queue.extend(elements(&element, "AXChildren"));
    }
    None
}

fn wait_for<T>(mut check: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + SEARCH_TIMEOUT;
    loop {
        if let Some(value) = check() {
            return Some(value);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(75));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_the_exact_local_conversation_route() {
        let id = "7f4048f5-e497-4911-9ab6-8be48f7c9778";
        assert_eq!(
            conversation_id_from_url(&format!("https://127.0.0.1:49152/c/{id}?project=test")),
            Uuid::parse_str(id).ok()
        );
        assert!(conversation_id_from_url(&format!("https://example.com/c/{id}")).is_none());
        assert!(conversation_id_from_url(&format!("https://localhost/c/{id}/extra")).is_none());
        assert!(conversation_id_from_url("https://localhost/c/new").is_none());
    }
}
