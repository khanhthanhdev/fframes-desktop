//! The gallery's capture mode: renders named pages in light and dark and saves PNGs.
//!
//! ```text
//! cargo run --manifest-path desktop/Cargo.toml -p fframes-studio --example ui_gallery -- \
//!     --capture <dir> [--appearance light|dark] [--page <name>]
//! ```
//!
//! Writes `<dir>/<appearance>/<page>.png` (or `<page>-<state>.png` for a page with several
//! fixture states) for each appearance (both when `--appearance` is left out) and each page
//! (all when `--page` is left out). The window is a real one: the page renders in it, then the
//! client area is read back with Win32 `PrintWindow(PW_RENDERFULLCONTENT)`, once per window
//! height while the page scrolls, and the pieces are stitched into one image. Capturing is only
//! implemented on Windows.
//!
//! A new page is a [`Page`] variant with its name, size and fixture states, and a branch in
//! `Gallery::render` that draws it.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{AsyncApp, Pixels, Size, WindowHandle, point, px, size};
use studio_theme::AppTheme;

use crate::Gallery;

/// How long a page gets to lay out, load icons and run its start-up animations.
const SETTLE: Duration = Duration::from_millis(1500);
/// How long a scrolled page gets to repaint.
const REPAINT: Duration = Duration::from_millis(400);

/// A capturable page of the gallery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// Every kit component the gallery shows, in its states.
    Kit,
    /// The Slides tab in its three review states and the timeline's stop markers.
    Slides,
    /// The Create panel: Blank (the default), Slide deck, and a vertical Sunset video chosen.
    Create,
    /// The phase 3 kit additions: ListRow, EmptyState, PanelSection, FormField, Divider,
    /// ResizeGrip, Progress, Spinner, Chip, Toast.
    Additions,
    /// The export panel: its settings, a running export, a finished and a failed one.
    Export,
    /// The settings window's frame on each of its pages.
    Settings,
    /// The first-run setup on each of its steps.
    Onboarding,
    /// The agent panel (phase 6) in its fixture states at 320 and 520 px, see `ui_gallery/agent.rs`.
    Agent,
    /// The Props tab's panel in its fixture states, see `ui_gallery/panels.rs`.
    Props,
    /// The Style tab's preset panel in its fixture states.
    Presets,
    /// The window chrome: header, notices, sidebar tab rail, transport and status bar.
    Shell,
    /// The start screen's project shelf: grid, list, a search and no recent projects.
    Home,
}

impl Page {
    pub const ALL: [Page; 12] = [
        Page::Kit,
        Page::Slides,
        Page::Create,
        Page::Additions,
        Page::Export,
        Page::Settings,
        Page::Onboarding,
        Page::Agent,
        Page::Props,
        Page::Presets,
        Page::Shell,
        Page::Home,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Page::Kit => "kit",
            Page::Slides => "slides",
            Page::Create => "create",
            Page::Additions => "additions",
            Page::Export => "export",
            Page::Settings => "settings",
            Page::Onboarding => "onboarding",
            Page::Agent => "agent",
            Page::Props => "props",
            Page::Presets => "presets",
            Page::Shell => "shell",
            Page::Home => "home",
        }
    }

    fn parse(name: &str) -> Option<Page> {
        Page::ALL.into_iter().find(|page| page.name() == name)
    }

    /// The window's content size. It fits a 1920 × 1200 screen at 150 %, except the Shell
    /// page: 1320 wide so the 1270 px Full transport fixture is not clipped, it fits at up to
    /// 145 %. A taller page is captured in scrolled pieces.
    fn size(self) -> Size<Pixels> {
        match self {
            Page::Shell => size(px(1320.), px(720.)),
            // The 1180 × 720 start screen under the gallery's title.
            Page::Home => size(px(1240.), px(840.)),
            // The 680 px panels under the gallery's title, in one piece: the composer's
            // popovers are anchored to the window, so pieces taken at two scroll offsets
            // would place them differently and the stitched page would show both.
            Page::Agent => size(px(1240.), px(780.)),
            _ => size(px(1240.), px(720.)),
        }
    }

    /// The fixture states, each saved as its own PNG. `""` is the page as it opens.
    pub fn states(self) -> &'static [&'static str] {
        match self {
            Page::Kit | Page::Slides | Page::Additions => &[""],
            Page::Create => &["", "slide-deck", "styled"],
            Page::Export => &["settings", "running", "done", "failed"],
            // In nav order, then the Agent page's CLI agents tab, a search, and its MCP servers
            // and Tools tabs; the index is
            // `render_settings_fixture`'s state.
            Page::Settings => &[
                "general",
                "appearance",
                "agent",
                "sdk",
                "presets",
                "export",
                "shortcuts",
                "about",
                "agent-cli",
                "search",
                "agent-mcp",
                "agent-tools",
                "agent-permissions",
                "agent-permissions-empty",
                "agent-permissions-no-project",
                "agent-cli-registry",
                "agent-cli-custom",
                "agent-mcp-import",
                "agent-mcp-registry",
                "agent-mcp-project",
                "agent-tools-project",
                "agent-cli-registry-loading",
                "agent-cli-registry-error",
                "agent-mcp-registry-loading",
                "agent-mcp-registry-error",
                "agent-mcp-linked",
                "agent-permissions-attaching",
                "agent-cli-linked",
                "sdk-upgrade",
                "about-update-ready",
            ],
            Page::Agent => &[
                "empty",
                "not-set-up",
                "working",
                "failed",
                "permission",
                "permission-pending",
                "question",
                "todo-queue",
                "steer-boundary",
                "steer-native",
                "changes",
                "idle-image",
                "code",
                "history",
                "permission-project",
                "tools-open",
                "approval-open",
            ],
            Page::Props => &[
                "fields",
                "disabled",
                "error",
                "not-editable",
                "no-selection",
                "no-project",
            ],
            Page::Presets => &[
                "list",
                "unapplied",
                "committed",
                "disabled",
                "empty",
                "no-project",
            ],
            Page::Shell => &["no-project", "export", "error"],
            Page::Home => &["grid", "list", "search", "empty"],
            Page::Onboarding => &[
                "welcome",
                "appearance",
                "sdk",
                "project",
                "agent",
                "permissions",
            ],
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Appearance {
    Light,
    Dark,
}

impl Appearance {
    fn name(self) -> &'static str {
        match self {
            Appearance::Light => "light",
            Appearance::Dark => "dark",
        }
    }

    fn theme(self) -> AppTheme {
        match self {
            Appearance::Light => AppTheme::Light,
            Appearance::Dark => AppTheme::Dark,
        }
    }
}

pub struct CaptureArgs {
    dir: PathBuf,
    appearances: Vec<Appearance>,
    pages: Vec<Page>,
}

impl CaptureArgs {
    /// The capture flags from the command line: `None` without `--capture` (the interactive
    /// gallery), an error message for flags it does not understand.
    pub fn from_env() -> Option<Result<CaptureArgs, String>> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.is_empty() {
            return None;
        }
        Some(Self::parse(&args))
    }

    fn parse(args: &[String]) -> Result<CaptureArgs, String> {
        let mut dir = None;
        let mut appearances = vec![Appearance::Light, Appearance::Dark];
        let mut pages = Page::ALL.to_vec();
        let mut args = args.iter();
        while let Some(flag) = args.next() {
            let mut value = || {
                args.next()
                    .ok_or_else(|| format!("{flag} needs a value"))
                    .cloned()
            };
            match flag.as_str() {
                "--capture" => dir = Some(PathBuf::from(value()?)),
                "--appearance" => {
                    appearances = match value()?.as_str() {
                        "light" => vec![Appearance::Light],
                        "dark" => vec![Appearance::Dark],
                        other => return Err(format!("unknown appearance `{other}`")),
                    }
                }
                "--page" => {
                    let name = value()?;
                    let page = Page::parse(&name).ok_or_else(|| {
                        let names: Vec<_> = Page::ALL.iter().map(|page| page.name()).collect();
                        format!("unknown page `{name}`; pages: {}", names.join(", "))
                    })?;
                    pages = vec![page];
                }
                other => return Err(format!("unknown argument `{other}`")),
            }
        }
        let dir = dir.ok_or("--appearance and --page need --capture <dir>")?;
        Ok(CaptureArgs {
            dir,
            appearances,
            pages,
        })
    }
}

/// Captures every requested page and state, then returns the written files.
pub async fn run(
    args: CaptureArgs,
    window: WindowHandle<Gallery>,
    cx: &mut AsyncApp,
) -> Result<Vec<PathBuf>, String> {
    let hwnd = window
        .update(cx, |_, window, _| native::window_id(window))
        .map_err(|error| error.to_string())??;
    let mut written = Vec::new();
    for appearance in &args.appearances {
        cx.update(|cx| studio_theme::set_preference(appearance.theme(), cx));
        let dir = args.dir.join(appearance.name());
        std::fs::create_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
        for page in &args.pages {
            for state in page.states() {
                let image = capture_page(&window, hwnd, *page, state, cx).await?;
                let file = if state.is_empty() {
                    format!("{}.png", page.name())
                } else {
                    format!("{}-{state}.png", page.name())
                };
                let path = dir.join(file);
                image
                    .save(&path)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                written.push(path);
            }
        }
    }
    Ok(written)
}

async fn capture_page(
    window: &WindowHandle<Gallery>,
    hwnd: isize,
    page: Page,
    state: &str,
    cx: &mut AsyncApp,
) -> Result<image::RgbaImage, String> {
    let state = state.to_owned();
    update(window, cx, move |gallery, window| {
        gallery.show_capture(page, &state);
        gallery.scroll.set_offset(point(px(0.), px(0.)));
        window.resize(page.size());
    })?;
    cx.background_executor().timer(SETTLE).await;

    let (viewport, max_scroll, scale) = update(window, cx, |gallery, window| {
        (
            window.viewport_size().height,
            gallery.scroll.max_offset().y.abs(),
            window.scale_factor(),
        )
    })?;
    let mut pieces = Vec::new();
    let mut top = px(0.);
    loop {
        update(window, cx, move |gallery, _| {
            gallery.scroll.set_offset(point(px(0.), -top))
        })?;
        cx.background_executor().timer(REPAINT).await;
        let piece = cx
            .background_executor()
            .spawn(async move { native::capture_client(hwnd) })
            .await?;
        pieces.push((top, piece));
        if top >= max_scroll {
            break;
        }
        top = (top + viewport).min(max_scroll);
    }
    update(window, cx, |gallery, _| {
        gallery.scroll.set_offset(point(px(0.), px(0.)))
    })?;
    Ok(stitch(&pieces, max_scroll, scale))
}

/// Runs `change` on the gallery and repaints it.
fn update<R>(
    window: &WindowHandle<Gallery>,
    cx: &mut AsyncApp,
    change: impl FnOnce(&mut Gallery, &mut gpui::Window) -> R,
) -> Result<R, String> {
    window
        .update(cx, |gallery, window, cx| {
            let result = change(gallery, window);
            cx.notify();
            result
        })
        .map_err(|error| error.to_string())
}

/// One image of the whole page from pieces captured at the given scroll offsets.
fn stitch(
    pieces: &[(Pixels, image::RgbaImage)],
    max_scroll: Pixels,
    scale: f32,
) -> image::RgbaImage {
    let (width, piece_height) = pieces
        .first()
        .map_or((0, 0), |(_, image)| image.dimensions());
    let height = piece_height + (f32::from(max_scroll) * scale).round() as u32;
    let mut out = image::RgbaImage::new(width, height);
    for (top, piece) in pieces {
        let y = (f32::from(*top) * scale).round() as i64;
        image::imageops::replace(&mut out, piece, 0, y);
    }
    out
}

#[cfg(windows)]
mod native {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
        DeleteDC, DeleteObject, GdiFlush, GetDC, ReleaseDC, SelectObject,
    };
    use windows_sys::Win32::Storage::Xps::{PW_CLIENTONLY, PrintWindow};
    use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

    /// `PW_RENDERFULLCONTENT`: read the DirectComposition content, not a GDI repaint.
    const PW_RENDERFULLCONTENT: u32 = 2;

    pub fn window_id(window: &gpui::Window) -> Result<isize, String> {
        let handle = HasWindowHandle::window_handle(window).map_err(|error| error.to_string())?;
        match handle.as_raw() {
            RawWindowHandle::Win32(handle) => Ok(handle.hwnd.get()),
            other => Err(format!("not a Win32 window: {other:?}")),
        }
    }

    /// The window's client area as it is on screen, in device pixels.
    pub fn capture_client(hwnd: isize) -> Result<image::RgbaImage, String> {
        let hwnd = hwnd as HWND;
        // SAFETY: plain GDI calls on a live window; every object created here is released
        // before returning, and the DIB section's bits are read only while it is alive.
        unsafe {
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if GetClientRect(hwnd, &mut rect) == 0 {
                return Err("GetClientRect failed".into());
            }
            let width = rect.right - rect.left;
            let height = rect.bottom - rect.top;
            if width <= 0 || height <= 0 {
                return Err(format!(
                    "the window has no client area ({width} × {height})"
                ));
            }
            let screen = GetDC(std::ptr::null_mut());
            let memory = CreateCompatibleDC(screen);
            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative: rows from the top.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(
                memory,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            );
            let result = if bitmap.is_null() || bits.is_null() {
                Err("CreateDIBSection failed".to_owned())
            } else {
                let previous = SelectObject(memory, bitmap);
                let printed = PrintWindow(hwnd, memory, PW_CLIENTONLY | PW_RENDERFULLCONTENT);
                GdiFlush();
                let result = if printed == 0 {
                    Err("PrintWindow failed".to_owned())
                } else {
                    let len = width as usize * height as usize * 4;
                    let bgra = std::slice::from_raw_parts(bits as *const u8, len);
                    let rgba = bgra
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .flat_map(|&[b, g, r, _]| [r, g, b, 255])
                        .collect();
                    image::RgbaImage::from_raw(width as u32, height as u32, rgba)
                        .ok_or_else(|| "the capture has the wrong size".to_owned())
                };
                SelectObject(memory, previous);
                DeleteObject(bitmap);
                result
            };
            DeleteDC(memory);
            ReleaseDC(std::ptr::null_mut(), screen);
            result
        }
    }
}

#[cfg(not(windows))]
mod native {
    pub fn window_id(_: &gpui::Window) -> Result<isize, String> {
        Err("--capture is only implemented on Windows".into())
    }

    pub fn capture_client(_: isize) -> Result<image::RgbaImage, String> {
        Err("--capture is only implemented on Windows".into())
    }
}
