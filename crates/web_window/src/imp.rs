use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;

use bevy::app::{App, AppExit, Last, Plugin, PluginsState};
use bevy::ecs::prelude::*;
use std::collections::HashMap;

use bevy::input::ButtonState;
use bevy::input::gamepad::{
    GamepadAxis, GamepadButton, GamepadConnection, GamepadConnectionEvent,
    RawGamepadAxisChangedEvent, RawGamepadButtonChangedEvent, RawGamepadEvent,
};
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput, NativeKey, NativeKeyCode};
use bevy::input::mouse::{MouseButton, MouseButtonInput, MouseMotion, MouseScrollUnit, MouseWheel};
use bevy::input::touch::TouchPhase;
use bevy::math::{DVec2, Vec2};
use bevy::window::{
    CursorEntered, CursorLeft, CursorMoved, CursorOptions, RawHandleWrapper,
    RawHandleWrapperHolder, Window, WindowCreated, WindowFocused, WindowResized,
    WindowScaleFactorChanged, WindowWrapper,
};
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle,
    WebOffscreenCanvasWindowHandle, WindowHandle,
};
use smol_str::SmolStr;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = r#"
// Pending input from the page, drained once per frame (see crates/web/www/PROTOCOL.md).
export function iw4_drain() {
  const q = globalThis.__iw4_inbox;
  if (!q || q.length === 0) return '';
  const s = JSON.stringify(q);
  q.length = 0;
  return s;
}
export function iw4_post_cursor(locked, visible) {
  self.postMessage({ type: 'cursor', locked, visible });
}
export function iw4_post_fps(fps, worst, memMB) {
  self.postMessage({ type: 'fps', fps, worst, memMB });
}
// requestAnimationFrame stops while the page is hidden, which would stall loading; a timer
// fallback keeps the engine ticking (at least 10 Hz). Whichever fires first runs the frame.
export function iw4_next_frame(cb) {
  let done = false;
  const run = (t) => { if (done) return; done = true; cb(t); };
  // Hidden page: no animation frames, so tick as fast as timers allow (loading is frame-sliced).
  if (globalThis.__iw4_hidden) { setTimeout(() => run(performance.now()), 0); return; }
  if (typeof self.requestAnimationFrame === 'function') self.requestAnimationFrame(run);
  setTimeout(() => run(performance.now()), 100);
}
export function iw4_now() { return performance.now(); }
export function iw4_log(text) { console.log(text); }
"#)]
extern "C" {
    fn iw4_drain() -> String;
    fn iw4_post_cursor(locked: bool, visible: bool);
    fn iw4_post_fps(fps: f64, worst: f64, mem_mb: f64);
    fn iw4_next_frame(cb: &Closure<dyn FnMut(f64)>);
    fn iw4_now() -> f64;
    fn iw4_log(text: &str);
}

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static SURFACE: RefCell<Option<Surface>> = const { RefCell::new(None) };
}

struct Surface {
    canvas: web_sys::OffscreenCanvas,
    width: u32,
    height: u32,
    scale: f32,
}

/// Selects the worker window backend; call before the engine's plugins are built.
pub fn enable(canvas: web_sys::OffscreenCanvas, width: u32, height: u32, scale: f32) {
    ENABLED.with(|e| e.set(true));
    SURFACE.with(|s| {
        *s.borrow_mut() = Some(Surface {
            canvas,
            width: width.max(1),
            height: height.max(1),
            scale: if scale > 0.0 { scale } else { 1.0 },
        });
    });
}

/// True once [`enable`] was called, i.e. the engine runs in the worker.
pub fn enabled() -> bool {
    ENABLED.with(Cell::get)
}

/// The OffscreenCanvas pointer wgpu needs; the `JsValue` is leaked so it outlives the window.
struct CanvasHandle(NonNull<c_void>);
// SAFETY: wasm here is single-threaded; the handle is only used on the worker's thread.
unsafe impl Send for CanvasHandle {}
unsafe impl Sync for CanvasHandle {}

impl HasWindowHandle for CanvasHandle {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = RawWindowHandle::WebOffscreenCanvas(WebOffscreenCanvasWindowHandle::new(self.0));
        // SAFETY: the pointer is a leaked JsValue and valid for the program's lifetime.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

impl HasDisplayHandle for CanvasHandle {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::web())
    }
}

pub struct WebWorkerWindowPlugin;

impl Plugin for WebWorkerWindowPlugin {
    fn build(&self, app: &mut App) {
        app.set_runner(runner)
            .add_systems(Last, post_cursor_options);
    }
}

fn post_cursor_options(options: Query<&CursorOptions, Changed<CursorOptions>>) {
    if let Some(options) = options.iter().last() {
        let locked = !matches!(options.grab_mode, bevy::window::CursorGrabMode::None);
        iw4_post_cursor(locked, options.visible);
    }
}

fn runner(app: App) -> AppExit {
    let app = Rc::new(RefCell::new(app));
    let tick: Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>> = Rc::new(RefCell::new(None));
    let again = tick.clone();
    let mut frames = 0u32;
    let mut window_start = iw4_now();
    let mut last_frame = window_start;
    let mut worst = 0.0f64;
    *tick.borrow_mut() = Some(Closure::new(move |_t: f64| {
        let now = iw4_now();
        worst = worst.max(now - last_frame);
        last_frame = now;
        let keep_going = {
            let mut app = app.borrow_mut();
            frame(&mut app)
        };
        frames += 1;
        if now - window_start >= 1000.0 {
            let fps = f64::from(frames) * 1000.0 / (now - window_start);
            let mem_mb = (core::arch::wasm32::memory_size(0) * 65536) as f64 / 1_048_576.0;
            iw4_post_fps(fps, worst, mem_mb);
            frames = 0;
            worst = 0.0;
            window_start = now;
        }
        if keep_going {
            if let Some(cb) = again.borrow().as_ref() {
                iw4_next_frame(cb);
            }
        } else {
            web_sys_log("web_window: app exited, frame loop stopped");
        }
    }));
    if let Some(cb) = tick.borrow().as_ref() {
        iw4_next_frame(cb);
    }
    // The browser owns the loop from here on; the closure keeps the app alive.
    AppExit::Success
}

fn web_sys_log(text: &str) {
    iw4_log(text);
}

/// One frame: plugin readiness, new windows, page input, then the app update.
/// Returns false once the app asked to exit.
fn frame(app: &mut App) -> bool {
    if app.plugins_state() != PluginsState::Cleaned {
        if app.plugins_state() == PluginsState::Ready {
            app.finish();
            app.cleanup();
        }
        // The renderer is still being created asynchronously; try again next frame.
        return true;
    }
    create_windows(app.world_mut());
    let raw = iw4_drain();
    if !raw.is_empty() {
        if let Ok(serde_json::Value::Array(events)) = serde_json::from_str(&raw) {
            for event in &events {
                apply_event(app.world_mut(), event);
            }
        }
    }
    app.update();
    match app.should_exit() {
        Some(exit) => {
            web_sys_log(&format!("web_window: exit requested: {exit:?}"));
            false
        }
        None => true,
    }
}

fn create_windows(world: &mut World) {
    let mut missing = world.query_filtered::<
        (Entity, Option<&RawHandleWrapperHolder>),
        (With<Window>, Without<RawHandleWrapper>),
    >();
    let todo: Vec<(Entity, Option<RawHandleWrapperHolder>)> = missing
        .iter(world)
        .map(|(entity, holder)| (entity, holder.cloned()))
        .collect();
    if todo.is_empty() {
        return;
    }
    let Some((canvas, width, height, scale)) = SURFACE.with(|s| {
        s.borrow()
            .as_ref()
            .map(|s| (s.canvas.clone(), s.width, s.height, s.scale))
    }) else {
        return;
    };
    for (entity, holder) in todo {
        canvas.set_width(width);
        canvas.set_height(height);
        let object: &'static mut JsValue = Box::leak(Box::new(JsValue::from(canvas.clone())));
        let ptr = NonNull::from(object).cast::<c_void>();
        let wrapper = WindowWrapper::new(CanvasHandle(ptr));
        let Ok(handle) = RawHandleWrapper::new(&wrapper) else {
            continue;
        };
        if let Some(mut window) = world.get_mut::<Window>(entity) {
            window.resolution.set_scale_factor(scale);
            window.resolution.set_physical_resolution(width, height);
            window.focused = true;
        }
        if let Some(holder) = holder {
            *holder.0.lock().unwrap() = Some(handle.clone());
        }
        world.entity_mut(entity).insert(handle);
        world.write_message(WindowCreated { window: entity });
    }
}

fn primary_window(world: &mut World) -> Option<Entity> {
    world
        .query_filtered::<Entity, With<bevy::window::PrimaryWindow>>()
        .iter(world)
        .next()
}

/// Per-pad state: the spawned `Gamepad` entity (kept across reconnects, like bevy_gilrs does, so
/// `GamepadSettings` survive), whether it is connected, and the last reported values.
struct PadState {
    entity: Entity,
    connected: bool,
    buttons: [f32; 17],
    axes: [f32; 4],
}

thread_local! {
    static PADS: RefCell<HashMap<u32, PadState>> = RefCell::new(HashMap::new());
}

const PAD_BUTTONS: [GamepadButton; 17] = [
    GamepadButton::South,
    GamepadButton::East,
    GamepadButton::West,
    GamepadButton::North,
    GamepadButton::LeftTrigger,
    GamepadButton::RightTrigger,
    GamepadButton::LeftTrigger2,
    GamepadButton::RightTrigger2,
    GamepadButton::Select,
    GamepadButton::Start,
    GamepadButton::LeftThumb,
    GamepadButton::RightThumb,
    GamepadButton::DPadUp,
    GamepadButton::DPadDown,
    GamepadButton::DPadLeft,
    GamepadButton::DPadRight,
    GamepadButton::Mode,
];

/// `(axis, sign)`; browser Y axes are down-positive, Bevy's are up-positive.
const PAD_AXES: [(GamepadAxis, f32); 4] = [
    (GamepadAxis::LeftStickX, 1.0),
    (GamepadAxis::LeftStickY, -1.0),
    (GamepadAxis::RightStickX, 1.0),
    (GamepadAxis::RightStickY, -1.0),
];

/// Feeds a page `pad` message into Bevy's raw gamepad events, the way bevy_gilrs does: connection
/// events go to both `RawGamepadEvent` and `GamepadConnectionEvent` (bevy_input then inserts
/// `Gamepad` + `Name` on the entity), changes to `RawGamepadEvent` and the typed raw messages.
fn apply_pad(world: &mut World, event: &serde_json::Value) {
    let index = event["index"].as_u64().unwrap_or(0) as u32;
    let connected = event["connected"].as_bool().unwrap_or(false);
    let id = event["id"].as_str().unwrap_or("Gamepad").to_string();
    let values = |name: &str| -> Vec<f32> {
        event[name]
            .as_array()
            .map(|a| a.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect())
            .unwrap_or_default()
    };

    let (entity, was_connected) = PADS.with(|pads| {
        let mut pads = pads.borrow_mut();
        let state = pads.entry(index).or_insert_with(|| PadState {
            entity: world.spawn_empty().id(),
            connected: false,
            buttons: [0.0; 17],
            axes: [0.0; 4],
        });
        (state.entity, state.connected)
    });

    let send_connection = |world: &mut World, connection: GamepadConnection| {
        let ev = GamepadConnectionEvent::new(entity, connection);
        world.write_message(RawGamepadEvent::from(ev.clone()));
        world.write_message(ev);
    };

    if !connected {
        if was_connected {
            send_connection(world, GamepadConnection::Disconnected);
            PADS.with(|pads| {
                if let Some(s) = pads.borrow_mut().get_mut(&index) {
                    s.connected = false;
                }
            });
        }
        return;
    }

    if !was_connected {
        // The browser id looks like "Xbox Controller (STANDARD GAMEPAD Vendor: 045e Product: 028e)".
        let hex_after = |key: &str| {
            id.find(key).and_then(|i| {
                let rest = id[i + key.len()..].trim_start();
                let hex: String = rest.chars().take_while(char::is_ascii_hexdigit).collect();
                u16::from_str_radix(&hex, 16).ok()
            })
        };
        let name = id.split(" (").next().unwrap_or(&id).trim().to_string();
        send_connection(
            world,
            GamepadConnection::Connected {
                name: if name.is_empty() { id.clone() } else { name },
                vendor_id: hex_after("Vendor:"),
                product_id: hex_after("Product:"),
            },
        );
        PADS.with(|pads| {
            if let Some(s) = pads.borrow_mut().get_mut(&index) {
                s.connected = true;
                s.buttons = [0.0; 17];
                s.axes = [0.0; 4];
            }
        });
    }

    let buttons = values("buttons");
    let axes = values("axes");
    let mut changed_buttons = Vec::new();
    let mut changed_axes = Vec::new();
    PADS.with(|pads| {
        let mut pads = pads.borrow_mut();
        let Some(state) = pads.get_mut(&index) else { return };
        for (i, &value) in buttons.iter().take(PAD_BUTTONS.len()).enumerate() {
            let value = value.clamp(0.0, 1.0);
            if (state.buttons[i] - value).abs() > f32::EPSILON {
                state.buttons[i] = value;
                changed_buttons.push((PAD_BUTTONS[i], value));
            }
        }
        for (i, &value) in axes.iter().take(PAD_AXES.len()).enumerate() {
            let (axis, sign) = PAD_AXES[i];
            let value = (value * sign).clamp(-1.0, 1.0);
            if (state.axes[i] - value).abs() > f32::EPSILON {
                state.axes[i] = value;
                changed_axes.push((axis, value));
            }
        }
    });
    for (button, value) in changed_buttons {
        let ev = RawGamepadButtonChangedEvent::new(entity, button, value);
        world.write_message(RawGamepadEvent::from(ev));
        world.write_message(ev);
    }
    for (axis, value) in changed_axes {
        let ev = RawGamepadAxisChangedEvent::new(entity, axis, value);
        world.write_message(RawGamepadEvent::from(ev));
        world.write_message(ev);
    }
}

fn apply_event(world: &mut World, event: &serde_json::Value) {
    if event["type"].as_str() == Some("pad") {
        apply_pad(world, event);
        return;
    }
    let Some(window) = primary_window(world) else {
        return;
    };
    let kind = event["type"].as_str().unwrap_or("");
    let num = |name: &str| event[name].as_f64().unwrap_or(0.0);
    let flag = |name: &str| event[name].as_bool().unwrap_or(false);
    let scale = world
        .get::<Window>(window)
        .map_or(1.0, |w| w.resolution.scale_factor());
    match kind {
        "key" => {
            let code = event["code"].as_str().unwrap_or("");
            let key = event["key"].as_str().unwrap_or("");
            let down = flag("down");
            let text = (down && key.chars().count() == 1).then(|| SmolStr::new(key));
            world.write_message(KeyboardInput {
                key_code: key_code(code),
                logical_key: logical_key(key),
                state: if down { ButtonState::Pressed } else { ButtonState::Released },
                text,
                repeat: flag("repeat"),
                window,
            });
        }
        "mousebtn" => {
            world.write_message(MouseButtonInput {
                button: match num("button") as u32 {
                    0 => MouseButton::Left,
                    1 => MouseButton::Middle,
                    2 => MouseButton::Right,
                    3 => MouseButton::Back,
                    4 => MouseButton::Forward,
                    other => MouseButton::Other(other as u16),
                },
                state: if flag("down") { ButtonState::Pressed } else { ButtonState::Released },
                window,
            });
        }
        "mousepos" => {
            let (x, y) = (num("x"), num("y"));
            let mut delta = None;
            if let Some(mut w) = world.get_mut::<Window>(window) {
                if let Some(prev) = w.physical_cursor_position() {
                    delta = Some(Vec2::new((x - f64::from(prev.x)) as f32, (y - f64::from(prev.y)) as f32) / scale);
                }
                w.set_physical_cursor_position(Some(DVec2::new(x, y)));
            }
            world.write_message(CursorMoved {
                window,
                position: Vec2::new(x as f32, y as f32) / scale,
                delta,
            });
        }
        "motion" => {
            world.write_message(MouseMotion {
                delta: Vec2::new(num("dx") as f32, num("dy") as f32),
            });
        }
        "wheel" => {
            let line = flag("line");
            world.write_message(MouseWheel {
                unit: if line { MouseScrollUnit::Line } else { MouseScrollUnit::Pixel },
                // Browsers report positive deltaY for scrolling down; winit/bevy want up positive.
                x: -(num("dx") as f32),
                y: -(num("dy") as f32),
                window,
                phase: TouchPhase::Moved,
            });
        }
        "resize" => {
            let (width, height) = (num("width").max(1.0) as u32, num("height").max(1.0) as u32);
            let new_scale = num("scale") as f32;
            let canvas = SURFACE.with(|s| {
                let mut s = s.borrow_mut();
                s.as_mut().map(|s| {
                    s.width = width;
                    s.height = height;
                    if new_scale > 0.0 {
                        s.scale = new_scale;
                    }
                    s.canvas.clone()
                })
            });
            if let Some(canvas) = canvas {
                canvas.set_width(width);
                canvas.set_height(height);
            }
            let mut scale_changed = None;
            if let Some(mut w) = world.get_mut::<Window>(window) {
                if new_scale > 0.0 && (new_scale - w.resolution.scale_factor()).abs() > f32::EPSILON {
                    w.resolution.set_scale_factor(new_scale);
                    scale_changed = Some(new_scale);
                }
                w.resolution.set_physical_resolution(width, height);
            }
            if let Some(new_scale) = scale_changed {
                world.write_message(WindowScaleFactorChanged {
                    window,
                    scale_factor: f64::from(new_scale),
                });
            }
            let scale = world
                .get::<Window>(window)
                .map_or(1.0, |w| w.resolution.scale_factor());
            world.write_message(WindowResized {
                window,
                width: width as f32 / scale,
                height: height as f32 / scale,
            });
        }
        "focus" => {
            let focused = flag("focused");
            if let Some(mut w) = world.get_mut::<Window>(window) {
                w.focused = focused;
            }
            world.write_message(WindowFocused { window, focused });
        }
        "cursorin" => {
            if flag("inside") {
                world.write_message(CursorEntered { window });
            } else {
                if let Some(mut w) = world.get_mut::<Window>(window) {
                    w.set_physical_cursor_position(None);
                }
                world.write_message(CursorLeft { window });
            }
        }
        _ => {}
    }
}

/// `KeyboardEvent.code` to Bevy's physical key code.
fn key_code(code: &str) -> KeyCode {
    use KeyCode::*;
    match code {
        "KeyA" => KeyA, "KeyB" => KeyB, "KeyC" => KeyC, "KeyD" => KeyD, "KeyE" => KeyE,
        "KeyF" => KeyF, "KeyG" => KeyG, "KeyH" => KeyH, "KeyI" => KeyI, "KeyJ" => KeyJ,
        "KeyK" => KeyK, "KeyL" => KeyL, "KeyM" => KeyM, "KeyN" => KeyN, "KeyO" => KeyO,
        "KeyP" => KeyP, "KeyQ" => KeyQ, "KeyR" => KeyR, "KeyS" => KeyS, "KeyT" => KeyT,
        "KeyU" => KeyU, "KeyV" => KeyV, "KeyW" => KeyW, "KeyX" => KeyX, "KeyY" => KeyY,
        "KeyZ" => KeyZ,
        "Digit0" => Digit0, "Digit1" => Digit1, "Digit2" => Digit2, "Digit3" => Digit3,
        "Digit4" => Digit4, "Digit5" => Digit5, "Digit6" => Digit6, "Digit7" => Digit7,
        "Digit8" => Digit8, "Digit9" => Digit9,
        "Numpad0" => Numpad0, "Numpad1" => Numpad1, "Numpad2" => Numpad2, "Numpad3" => Numpad3,
        "Numpad4" => Numpad4, "Numpad5" => Numpad5, "Numpad6" => Numpad6, "Numpad7" => Numpad7,
        "Numpad8" => Numpad8, "Numpad9" => Numpad9, "NumpadAdd" => NumpadAdd,
        "NumpadSubtract" => NumpadSubtract, "NumpadMultiply" => NumpadMultiply,
        "NumpadDivide" => NumpadDivide, "NumpadDecimal" => NumpadDecimal,
        "NumpadEnter" => NumpadEnter, "NumLock" => NumLock,
        "F1" => F1, "F2" => F2, "F3" => F3, "F4" => F4, "F5" => F5, "F6" => F6, "F7" => F7,
        "F8" => F8, "F9" => F9, "F10" => F10, "F11" => F11, "F12" => F12,
        "ArrowUp" => ArrowUp, "ArrowDown" => ArrowDown, "ArrowLeft" => ArrowLeft,
        "ArrowRight" => ArrowRight,
        "Enter" => Enter, "Escape" => Escape, "Space" => Space, "Tab" => Tab,
        "Backspace" => Backspace, "Delete" => Delete, "Insert" => Insert, "Home" => Home,
        "End" => End, "PageUp" => PageUp, "PageDown" => PageDown, "CapsLock" => CapsLock,
        "ShiftLeft" => ShiftLeft, "ShiftRight" => ShiftRight,
        "ControlLeft" => ControlLeft, "ControlRight" => ControlRight,
        "AltLeft" => AltLeft, "AltRight" => AltRight,
        "MetaLeft" => SuperLeft, "MetaRight" => SuperRight,
        "Minus" => Minus, "Equal" => Equal, "BracketLeft" => BracketLeft,
        "BracketRight" => BracketRight, "Backslash" => Backslash, "Semicolon" => Semicolon,
        "Quote" => Quote, "Backquote" => Backquote, "Comma" => Comma, "Period" => Period,
        "Slash" => Slash, "IntlBackslash" => IntlBackslash, "ContextMenu" => ContextMenu,
        "PrintScreen" => PrintScreen, "ScrollLock" => ScrollLock, "Pause" => Pause,
        _ => Unidentified(NativeKeyCode::Unidentified),
    }
}

/// `KeyboardEvent.key` to Bevy's logical key.
fn logical_key(key: &str) -> Key {
    match key {
        "Enter" => Key::Enter,
        "Escape" => Key::Escape,
        " " => Key::Space,
        "Tab" => Key::Tab,
        "Backspace" => Key::Backspace,
        "Delete" => Key::Delete,
        "Insert" => Key::Insert,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        "ArrowUp" => Key::ArrowUp,
        "ArrowDown" => Key::ArrowDown,
        "ArrowLeft" => Key::ArrowLeft,
        "ArrowRight" => Key::ArrowRight,
        "Shift" => Key::Shift,
        "Control" => Key::Control,
        "Alt" => Key::Alt,
        "Meta" => Key::Super,
        "CapsLock" => Key::CapsLock,
        "F1" => Key::F1,
        "F2" => Key::F2,
        "F3" => Key::F3,
        "F4" => Key::F4,
        "F5" => Key::F5,
        "F6" => Key::F6,
        "F7" => Key::F7,
        "F8" => Key::F8,
        "F9" => Key::F9,
        "F10" => Key::F10,
        "F11" => Key::F11,
        "F12" => Key::F12,
        other if other.chars().count() == 1 => Key::Character(SmolStr::new(other)),
        other => Key::Unidentified(NativeKey::Web(SmolStr::new(other))),
    }
}
