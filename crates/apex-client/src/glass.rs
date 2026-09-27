//! The floating sidebar on Liquid Glass, as Reflect's peek sidebar is.
//! Glass shows what is behind it, and what is behind the sidebar is
//! drawn by gpui in the same layer as the sidebar itself (or is a page,
//! a native view above all of it), so the sidebar cannot be glass in the
//! window it floats over. It is drawn instead in a window of its own: a
//! borderless panel, a child of the main window (it moves with it, and
//! stays over it), its gpui view transparent over an `NSGlassEffectView`
//! -- which, as a menu's does, shows the main window through it, pages
//! and all, with no hole to cut.
//!
//! The panel is the card alone. It covers the main window's own buttons,
//! so it has buttons of its own in their place, which act on the main
//! window; the main window's are hidden while it is out. Its clicks that
//! act on the window (a session chosen, the header dragged) go to the
//! main window's (`Acme::on_main`). Where glass is not to be had (before
//! macOS 26), the sidebar floats in the window as it did.

use gpui::prelude::*;
use gpui::{div, px, Bounds, Context, Pixels, WeakEntity, Window};

use crate::app::Acme;

/// The panel's gpui view: the sidebar's card, drawn from the app's state.
pub struct SidebarPanel {
    acme: WeakEntity<Acme>,
    /// The panel's NSWindow.
    pub panel: usize,
}

impl SidebarPanel {
    pub fn new(acme: gpui::Entity<Acme>, panel: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // drawn again whenever the app is; gone when it is
        cx.observe(&acme, |_, _, cx| cx.notify()).detach();
        cx.observe_release_in(&acme, window, |_, _, window, _| window.remove_window()).detach();
        SidebarPanel { acme: acme.downgrade(), panel }
    }
}

impl Render for SidebarPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let card = self.acme.upgrade().map(|a| a.update(cx, |a, cx| a.sidebar(true, cx).into_any_element()));
        div().size_full().children(card)
    }
}

/// The panel, once it is made.
///
/// It stands where the card does, always, a child of the main window,
/// the window itself never hidden or faded: a window no one can see has
/// gpui stop drawing it (its display link goes with its visibility), and
/// it would start again only when AppKit got round to saying it is seen.
/// What comes and goes is its content -- the glass, the card drawn on it,
/// the buttons -- slid and faded by Core Animation, as Reflect's is: one
/// animation handed to the render server, smooth however long the app's
/// own frames take. Put away, its content is clear and the clicks pass
/// through it.
pub struct Glass {
    panel: usize,
    main: usize,
    /// A child of the main window (in front, moving with it).
    attached: bool,
    /// The frame on screen the window server was last told.
    frame: Option<[f64; 4]>,
    /// Out (shown), as last animated.
    out: bool,
    /// Put away since: its shadow goes once the content has faded.
    away_at: Option<std::time::Instant>,
}

pub enum GlassState {
    Untried,
    Opening,
    Ready(Glass),
    /// No glass here: the sidebar floats in the window.
    Unavailable,
}

impl Acme {
    /// The floating sidebar `slide` of the way in (None: away): its panel
    /// placed and shown, or put away. True when the panel has the sidebar,
    /// so the window draws none of its own.
    pub fn glass_tick(&mut self, slide: Option<f32>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let pinned = self.sidebar_shown();
        match &mut self.glass {
            // pinned: the sidebar is the window's own
            GlassState::Ready(g) if pinned => {
                detach(g);
                false
            }
            // floating: the panel always there, where the card stands; its
            // content slid and faded in or out when the sidebar comes or
            // goes (`slide` is the in-window sidebar's; the panel's own
            // animation is Core Animation's)
            GlassState::Ready(g) => {
                let _ = slide;
                let inset = px(crate::sidebar::INSET);
                let size = window.viewport_size();
                let r = Bounds::new(gpui::point(inset, inset), gpui::size(px(crate::shell::SIDEBAR_W) - inset * 2., size.height - inset * 2.));
                place(g, window, r);
                let out = self.sidebar_out;
                if out != g.out {
                    animate(g, out);
                    g.out = out;
                    g.away_at = (!out).then(std::time::Instant::now);
                    if !out {
                        // the shadow, once the content has faded
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(std::time::Duration::from_millis(200)).await;
                            let _ = this.update(cx, |a, _| {
                                if let GlassState::Ready(g) = &mut a.glass {
                                    if !g.out && g.away_at.is_some_and(|t| t.elapsed() >= std::time::Duration::from_millis(180)) {
                                        shadow(g, false);
                                    }
                                }
                            });
                        })
                        .detach();
                    }
                }
                true
            }
            // made as soon as the sidebar floats, not when it is first
            // brought
            GlassState::Untried if !pinned => {
                if !supported() {
                    self.glass = GlassState::Unavailable;
                    return false;
                }
                let Some(main) = ns_window(window) else {
                    self.glass = GlassState::Unavailable;
                    return false;
                };
                self.glass = GlassState::Opening;
                let me = cx.entity();
                cx.defer(move |cx| {
                    let options = gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(Bounds::new(gpui::point(px(0.), px(0.)), gpui::size(px(200.), px(400.))))),
                        titlebar: None,
                        focus: false,
                        show: false,
                        kind: gpui::WindowKind::PopUp,
                        is_movable: false,
                        is_resizable: false,
                        is_minimizable: false,
                        window_background: gpui::WindowBackgroundAppearance::Transparent,
                        ..Default::default()
                    };
                    let me2 = me.clone();
                    let opened = cx.open_window(options, move |window, cx| {
                        let panel = install(window, main).unwrap_or(0);
                        cx.new(|cx| SidebarPanel::new(me2, panel, window, cx))
                    });
                    let state = match opened {
                        Ok(handle) => match handle.read_with(cx, |p, _| p.panel) {
                            Ok(panel) if panel != 0 => GlassState::Ready(Glass { panel, main, attached: false, frame: None, out: false, away_at: None }),
                            _ => {
                                let _ = handle.update(cx, |_, window, _| window.remove_window());
                                GlassState::Unavailable
                            }
                        },
                        Err(_) => GlassState::Unavailable,
                    };
                    me.update(cx, |a, cx| {
                        a.glass = state;
                        cx.notify();
                    });
                });
                false
            }
            _ => false,
        }
    }

    /// Whether the floating sidebar is drawn in its panel, on glass.
    pub fn on_glass(&self) -> bool {
        matches!(self.glass, GlassState::Ready(_))
    }

    /// `f` on the main window, from a click that may have come in the
    /// panel's: after this event, since the main window's view is the
    /// app itself and is being updated now.
    pub fn on_main(&self, cx: &mut Context<Self>, f: impl FnOnce(&mut Acme, &mut Window, &mut Context<Acme>) + 'static) {
        let Some(h) = self.main_window else { return };
        cx.defer(move |cx| {
            let _ = h.update(cx, |acme, window, cx| f(acme, window, cx));
        });
    }

    /// The main window moved by a press on the sidebar's header, in
    /// whichever window the sidebar is.
    pub fn drag_main(&self, window: &Window) {
        match &self.glass {
            GlassState::Ready(g) => drag(g.main),
            _ => window.start_window_move(),
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use objc::runtime::{Class, Object};
    use objc::{class, msg_send, sel, sel_impl};

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct P {
        pub x: f64,
        pub y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct R {
        pub origin: P,
        pub size: P,
    }

    /// NSWindowBelow, NSWindowAbove
    const BELOW: i64 = -1;
    const ABOVE: i64 = 1;
    /// NSViewWidthSizable | NSViewHeightSizable; NSViewMaxXMargin |
    /// NSViewMinYMargin (pinned to the top left of an unflipped view)
    const SIZABLE: u64 = 2 | 16;
    const TOP_LEFT: u64 = 4 | 8;

    pub fn supported() -> bool {
        Class::get("NSGlassEffectView").is_some()
    }

    /// Window `window`'s NSWindow, and its gpui view.
    fn native(window: &Window) -> Option<(*mut Object, *mut Object)> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let h = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(h) = h.as_raw() else { return None };
        let view = h.ns_view.as_ptr() as *mut Object;
        // SAFETY: gpui's own view, alive while the window is
        let ns_window: *mut Object = unsafe { msg_send![view, window] };
        (!ns_window.is_null()).then_some((ns_window, view))
    }

    pub fn ns_window(window: &Window) -> Option<usize> {
        native(window).map(|(w, _)| w as usize)
    }

    /// The panel made glass: an `NSGlassEffectView` under gpui's view,
    /// rounded as the card is; the window's buttons, standing in for the
    /// main window's (`main`). Its NSWindow, or None.
    pub fn install(window: &Window, main: usize) -> Option<usize> {
        let (panel, view) = native(window)?;
        let main = main as *mut Object;
        // SAFETY: AppKit on the main thread, on windows and views alive
        // while the panel is; the glass and the buttons are retained by
        // the views they are added to.
        unsafe {
            let content: *mut Object = msg_send![view, superview];
            if content.is_null() {
                return None;
            }
            let bounds: R = msg_send![content, bounds];
            let cls = Class::get("NSGlassEffectView")?;
            let glass: *mut Object = msg_send![cls, alloc];
            let glass: *mut Object = msg_send![glass, initWithFrame: bounds];
            let _: () = msg_send![glass, setCornerRadius: 10.0f64];
            let _: () = msg_send![glass, setAutoresizingMask: SIZABLE];
            let nil: *mut Object = std::ptr::null_mut();
            let _: () = msg_send![content, addSubview: glass positioned: BELOW relativeTo: nil];
            let _: () = msg_send![glass, release];
            // a panel that takes no keys from the main window unless a
            // view of it asks, stays when the app is in the back (its
            // parent hides it), and casts the card's shadow
            let _: () = msg_send![panel, setBecomesKeyOnlyIfNeeded: true];
            let _: () = msg_send![panel, setHidesOnDeactivate: false];
            // put away to begin with: the content clear, no shadow, the
            // clicks through
            let _: () = msg_send![content, setWantsLayer: true];
            let layer: *mut Object = msg_send![content, layer];
            if !layer.is_null() {
                let _: () = msg_send![layer, setOpacity: 0.0f32];
            }
            let _: () = msg_send![panel, setHasShadow: false];
            let _: () = msg_send![panel, setIgnoresMouseEvents: true];
            let level: i64 = msg_send![main, level];
            let _: () = msg_send![panel, setLevel: level];
            // the window's buttons where the main window's stand, less
            // the card's inset: acting on the main window
            let inset = crate::sidebar::INSET as f64;
            let height = bounds.size.y;
            let actions = [sel!(performClose:), sel!(performMiniaturize:), sel!(toggleFullScreen:)];
            for (kind, action) in actions.into_iter().enumerate() {
                let theirs: *mut Object = msg_send![main, standardWindowButton: kind as u64];
                if theirs.is_null() {
                    continue;
                }
                let at: R = msg_send![theirs, frame];
                let mask: u64 = msg_send![main, styleMask];
                let button: *mut Object = msg_send![class!(NSWindow), standardWindowButton: kind as u64 forStyleMask: mask];
                if button.is_null() {
                    continue;
                }
                let top = crate::main_lights_y() as f64 - inset;
                let frame = R { origin: P { x: at.origin.x - inset, y: height - top - at.size.y }, size: at.size };
                let _: () = msg_send![button, setFrame: frame];
                let _: () = msg_send![button, setAutoresizingMask: TOP_LEFT];
                let _: () = msg_send![button, setTarget: main];
                let _: () = msg_send![button, setAction: action];
                let _: () = msg_send![content, addSubview: button positioned: ABOVE relativeTo: nil];
            }
            Some(panel as usize)
        }
    }

    /// The panel at `r` (in the main window's view, top left first),
    /// over the main window. Only a change is sent.
    pub fn place(g: &mut Glass, main: &Window, r: Bounds<Pixels>) {
        let Some((main_window, main_view)) = native(main) else { return };
        let panel = g.panel as *mut Object;
        let local = R { origin: P { x: f32::from(r.origin.x) as f64, y: f32::from(r.origin.y) as f64 }, size: P { x: f32::from(r.size.width) as f64, y: f32::from(r.size.height) as f64 } };
        // SAFETY: AppKit on the main thread; both windows alive
        unsafe {
            let nil: *mut Object = std::ptr::null_mut();
            let in_window: R = msg_send![main_view, convertRect: local toView: nil];
            let on_screen: R = msg_send![main_window, convertRectToScreen: in_window];
            let frame = [on_screen.origin.x, on_screen.origin.y, on_screen.size.x, on_screen.size.y];
            if g.frame != Some(frame) {
                let resized = g.frame.is_none_or(|f| f[2] != frame[2] || f[3] != frame[3]);
                let _: () = msg_send![panel, setFrame: on_screen display: false];
                if resized {
                    let _: () = msg_send![panel, invalidateShadow];
                }
                g.frame = Some(frame);
            }
            if !g.attached {
                let _: () = msg_send![main_window, addChildWindow: panel ordered: ABOVE];
                let _: () = msg_send![panel, orderFront: nil];
                g.attached = true;
            }
        }
    }

    fn ns_string(s: &str) -> *mut Object {
        let c = std::ffi::CString::new(s).unwrap_or_default();
        // SAFETY: an autoreleased NSString from a C string
        unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
    }

    /// The content slid and faded in (`out`) or away, by Core Animation:
    /// in over 0.16 s easing out from 24 pixels to the left, away over
    /// 0.14 s easing in -- the in-window sidebar's slide. The clicks are
    /// the panel's from the start of the way in, and pass through it from
    /// the start of the way out.
    pub fn animate(g: &mut Glass, out: bool) {
        let panel = g.panel as *mut Object;
        // SAFETY: AppKit and Core Animation on the main thread, on the
        // panel's own content view and layer
        unsafe {
            let content: *mut Object = msg_send![panel, contentView];
            if content.is_null() {
                return;
            }
            let layer: *mut Object = msg_send![content, layer];
            if layer.is_null() {
                return;
            }
            let (o0, o1, x0, x1, dur, curve) = if out { (0.0f32, 1.0f32, -24.0f64, 0.0f64, 0.16f64, "easeOut") } else { (1.0, 0.0, 0.0, -24.0, 0.14, "easeIn") };
            // where it was drawn when turned about mid-way: from there
            let shown: *mut Object = msg_send![layer, presentationLayer];
            let o0 = if shown.is_null() { o0 } else { msg_send![shown, opacity] };
            let timing: *mut Object = msg_send![class!(CAMediaTimingFunction), functionWithName: ns_string(curve)];
            let fade: *mut Object = msg_send![class!(CABasicAnimation), animationWithKeyPath: ns_string("opacity")];
            let from: *mut Object = msg_send![class!(NSNumber), numberWithFloat: o0];
            let to: *mut Object = msg_send![class!(NSNumber), numberWithFloat: o1];
            let _: () = msg_send![fade, setFromValue: from];
            let _: () = msg_send![fade, setToValue: to];
            let _: () = msg_send![fade, setDuration: dur];
            let _: () = msg_send![fade, setTimingFunction: timing];
            let slide: *mut Object = msg_send![class!(CABasicAnimation), animationWithKeyPath: ns_string("transform.translation.x")];
            let from: *mut Object = msg_send![class!(NSNumber), numberWithDouble: x0];
            let to: *mut Object = msg_send![class!(NSNumber), numberWithDouble: x1];
            let _: () = msg_send![slide, setFromValue: from];
            let _: () = msg_send![slide, setToValue: to];
            let _: () = msg_send![slide, setDuration: dur];
            let _: () = msg_send![slide, setTimingFunction: timing];
            // the model where it ends (the slide's end is the identity:
            // away, it is clear anyway)
            let _: () = msg_send![class!(CATransaction), begin];
            let _: () = msg_send![class!(CATransaction), setDisableActions: true];
            let _: () = msg_send![layer, setOpacity: o1];
            let _: () = msg_send![class!(CATransaction), commit];
            let _: () = msg_send![layer, addAnimation: fade forKey: ns_string("apex-fade")];
            let _: () = msg_send![layer, addAnimation: slide forKey: ns_string("apex-slide")];
            let _: () = msg_send![panel, setIgnoresMouseEvents: !out];
        }
        if out {
            shadow(g, true);
        }
    }

    /// The card's shadow on or off (the window's, from its content).
    pub fn shadow(g: &mut Glass, on: bool) {
        let panel = g.panel as *mut Object;
        // SAFETY: AppKit on the main thread
        unsafe {
            let _: () = msg_send![panel, setHasShadow: on];
            if on {
                let _: () = msg_send![panel, invalidateShadow];
            }
        }
    }

    /// The panel gone altogether (the sidebar pinned).
    pub fn detach(g: &mut Glass) {
        if !g.attached {
            return;
        }
        let (panel, main) = (g.panel as *mut Object, g.main as *mut Object);
        // SAFETY: AppKit on the main thread; both windows alive
        unsafe {
            let nil: *mut Object = std::ptr::null_mut();
            let _: () = msg_send![main, removeChildWindow: panel];
            let _: () = msg_send![panel, orderOut: nil];
        }
        g.attached = false;
    }

    /// The main window moved as its title bar would be, by the press
    /// being handled (in the panel).
    pub fn drag(main: usize) {
        // SAFETY: AppKit on the main thread, during the press's handling
        unsafe {
            let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
            let event: *mut Object = msg_send![app, currentEvent];
            if !event.is_null() {
                let _: () = msg_send![main as *mut Object, performWindowDragWithEvent: event];
            }
        }
    }
}

#[cfg(target_os = "macos")]
use mac::{animate, detach, drag, install, ns_window, place, shadow, supported};

#[cfg(not(target_os = "macos"))]
fn supported() -> bool {
    false
}
#[cfg(not(target_os = "macos"))]
fn ns_window(_: &Window) -> Option<usize> {
    None
}
#[cfg(not(target_os = "macos"))]
fn install(_: &Window, _: usize) -> Option<usize> {
    None
}
#[cfg(not(target_os = "macos"))]
fn place(_: &mut Glass, _: &Window, _: Bounds<Pixels>) {}
#[cfg(not(target_os = "macos"))]
fn animate(_: &mut Glass, _: bool) {}
#[cfg(not(target_os = "macos"))]
fn shadow(_: &mut Glass, _: bool) {}
#[cfg(not(target_os = "macos"))]
fn detach(_: &mut Glass) {}
#[cfg(not(target_os = "macos"))]
fn drag(_: usize) {}
