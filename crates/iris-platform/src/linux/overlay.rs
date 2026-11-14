use crate::OverlayWindowBackend;
use iris_core::models::IntRect;
use rustix::fs::{memfd_create, MemfdFlags};
use rustix::mm::{mmap, munmap, MapFlags, ProtFlags};
use std::ffi::c_void;
use std::os::fd::{AsFd, OwnedFd};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_region, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection as WlConnection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1, zwlr_layer_surface_v1,
};
use x11rb::connection::{Connection as X11ConnTrait, RequestConnection};
use x11rb::protocol::shape::{self, ConnectionExt as ShapeExt};
use x11rb::protocol::xproto::{
    AtomEnum, ColormapAlloc, ConfigureWindowAux, ConnectionExt as XprotoExt, CreateGCAux,
    CreateWindowAux, EventMask, Gcontext, ImageFormat, PropMode, StackMode, VisualClass, Window,
    WindowClass,
};
use x11rb::rust_connection::RustConnection;

// ============================================================================
// Primary Wayland `zwlr_layer_shell_v1` click-through overlay (KDE KWin, Sway, Hyprland)
// ============================================================================

struct WlState {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    configured_serial: Option<u32>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for WlState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &WlConnection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor =
                        Some(registry.bind::<wl_compositor::WlCompositor, _, _>(
                            name,
                            version.min(5),
                            qh,
                            (),
                        ));
                }
                "wl_shm" => {
                    state.shm =
                        Some(registry.bind::<wl_shm::WlShm, _, _>(name, version.min(1), qh, ()));
                }
                "zwlr_layer_shell_v1" => {
                    state.layer_shell = Some(
                        registry.bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1, _, _>(
                            name,
                            version.min(4),
                            qh,
                            (),
                        ),
                    );
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for WlState {
    fn event(
        _: &mut Self,
        _: &wl_compositor::WlCompositor,
        _: wl_compositor::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm::WlShm, ()> for WlState {
    fn event(
        _: &mut Self,
        _: &wl_shm::WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for WlState {
    fn event(
        _: &mut Self,
        _: &wl_shm_pool::WlShmPool,
        _: wl_shm_pool::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for WlState {
    fn event(
        _: &mut Self,
        _: &wl_buffer::WlBuffer,
        _: wl_buffer::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for WlState {
    fn event(
        _: &mut Self,
        _: &wl_surface::WlSurface,
        _: wl_surface::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_region::WlRegion, ()> for WlState {
    fn event(
        _: &mut Self,
        _: &wl_region::WlRegion,
        _: wl_region::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_layer_shell_v1::ZwlrLayerShellV1, ()> for WlState {
    fn event(
        _: &mut Self,
        _: &zwlr_layer_shell_v1::ZwlrLayerShellV1,
        _: zwlr_layer_shell_v1::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for WlState {
    fn event(
        state: &mut Self,
        layer_surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure { serial, .. } = event {
            layer_surface.ack_configure(serial);
            state.configured_serial = Some(serial);
        }
    }
}

struct ShmMapping {
    _fd: OwnedFd,
    ptr: *mut c_void,
    byte_len: usize,
    pool: wl_shm_pool::WlShmPool,
    buffer: wl_buffer::WlBuffer,
    width: i32,
    height: i32,
}

unsafe impl Send for ShmMapping {}

impl Drop for ShmMapping {
    fn drop(&mut self) {
        self.buffer.destroy();
        self.pool.destroy();
        unsafe {
            let _ = munmap(self.ptr, self.byte_len);
        }
    }
}

struct WaylandLayerOverlay {
    conn: WlConnection,
    queue: EventQueue<WlState>,
    state: WlState,
    qh: QueueHandle<WlState>,
    surface: wl_surface::WlSurface,
    layer_surface: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    empty_region: wl_region::WlRegion,
    shm_map: Option<ShmMapping>,
    cur_bounds: IntRect,
    visible: bool,
}

impl WaylandLayerOverlay {
    fn try_new() -> anyhow::Result<Self> {
        let conn = WlConnection::connect_to_env()?;
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        let display = conn.display();
        let _registry = display.get_registry(&qh, ());

        let mut state = WlState {
            compositor: None,
            shm: None,
            layer_shell: None,
            configured_serial: None,
        };
        queue.roundtrip(&mut state)?;

        let compositor = state
            .compositor
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Wayland compositor missing wl_compositor"))?;
        let _shm = state
            .shm
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Wayland compositor missing wl_shm"))?;
        let layer_shell = state.layer_shell.clone().ok_or_else(|| {
            anyhow::anyhow!("Wayland compositor does not advertise zwlr_layer_shell_v1")
        })?;

        let surface = compositor.create_surface(&qh, ());
        let empty_region = compositor.create_region(&qh, ());
        surface.set_input_region(Some(&empty_region));

        let layer_surface = layer_shell.get_layer_surface(
            &surface,
            None,
            zwlr_layer_shell_v1::Layer::Overlay,
            "sc-overlay".to_string(),
            &qh,
            (),
        );
        layer_surface.set_anchor(
            zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Left,
        );
        layer_surface.set_exclusive_zone(-1);
        layer_surface.set_keyboard_interactivity(
            zwlr_layer_surface_v1::KeyboardInteractivity::None,
        );
        layer_surface.set_size(1, 1);
        surface.commit();
        queue.roundtrip(&mut state)?;

        Ok(Self {
            conn,
            queue,
            state,
            qh,
            surface,
            layer_surface,
            empty_region,
            shm_map: None,
            cur_bounds: IntRect::new(0, 0, 1, 1),
            visible: false,
        })
    }

    fn ensure_shm(&mut self, width: i32, height: i32) -> anyhow::Result<()> {
        if let Some(m) = &self.shm_map {
            if m.width == width && m.height == height {
                return Ok(());
            }
        }
        self.shm_map = None;
        let byte_len = (width as usize) * (height as usize) * 4;
        let fd = memfd_create("sc-overlay-wl-shm", MemfdFlags::CLOEXEC)?;
        rustix::fs::ftruncate(&fd, byte_len as u64)?;
        let ptr = unsafe {
            mmap(
                std::ptr::null_mut(),
                byte_len,
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::SHARED,
                &fd,
                0,
            )?
        };
        let shm = self.state.shm.as_ref().unwrap();
        let pool = shm.create_pool(fd.as_fd(), byte_len as i32, &self.qh, ());
        let buffer = pool.create_buffer(
            0,
            width,
            height,
            width * 4,
            wl_shm::Format::Argb8888,
            &self.qh,
            (),
        );
        self.shm_map = Some(ShmMapping {
            _fd: fd,
            ptr,
            byte_len,
            pool,
            buffer,
            width,
            height,
        });
        Ok(())
    }

    fn present(&mut self, bounds: IntRect, bgra_premul: &[u8]) -> anyhow::Result<()> {
        if bounds.width <= 0 || bounds.height <= 0 {
            self.hide();
            return Ok(());
        }
        let expected = (bounds.width as usize) * (bounds.height as usize) * 4;
        if bgra_premul.len() < expected {
            return Ok(());
        }

        if bounds != self.cur_bounds {
            self.layer_surface
                .set_margin(bounds.y, 0, 0, bounds.x);
            self.layer_surface
                .set_size(bounds.width as u32, bounds.height as u32);
            self.surface.set_input_region(Some(&self.empty_region));
            self.surface.commit();
            let _ = self.queue.roundtrip(&mut self.state);
            self.cur_bounds = bounds;
        }

        self.ensure_shm(bounds.width, bounds.height)?;
        if let Some(m) = &self.shm_map {
            unsafe {
                std::ptr::copy_nonoverlapping(bgra_premul.as_ptr(), m.ptr as *mut u8, expected);
            }
            self.surface.set_input_region(Some(&self.empty_region));
            self.surface.attach(Some(&m.buffer), 0, 0);
            self.surface
                .damage_buffer(0, 0, bounds.width, bounds.height);
            self.surface.commit();
            let _ = self.conn.flush();
            let _ = self.queue.dispatch_pending(&mut self.state);
            self.visible = true;
        }
        Ok(())
    }

    fn hide(&mut self) {
        if self.visible {
            self.surface.attach(None, 0, 0);
            self.surface.commit();
            let _ = self.conn.flush();
            let _ = self.queue.dispatch_pending(&mut self.state);
            self.visible = false;
        }
    }
}

// ============================================================================
// Fallback X11 / XWayland / Gamescope 32-bit ARGB override-redirect overlay
// ============================================================================

struct X11ArgbOverlay {
    conn: RustConnection,
    win: Window,
    gc: Gcontext,
    cur_bounds: IntRect,
    mapped: bool,
}

impl X11ArgbOverlay {
    fn try_new() -> anyhow::Result<Self> {
        let (conn, screen_num) = x11rb::connect(None)?;
        let screen = conn
            .setup()
            .roots
            .get(screen_num)
            .ok_or_else(|| anyhow::anyhow!("Invalid X11 screen"))?;
        let root = screen.root;

        // Find a 32-bit TrueColor ARGB visual
        let mut visual_id = None;
        for depth in &screen.allowed_depths {
            if depth.depth == 32 {
                for vis in &depth.visuals {
                    if vis.class == VisualClass::TRUE_COLOR {
                        visual_id = Some(vis.visual_id);
                        break;
                    }
                }
            }
            if visual_id.is_some() {
                break;
            }
        }
        let visual =
            visual_id.ok_or_else(|| anyhow::anyhow!("No 32-bit ARGB TrueColor visual on X11"))?;

        let colormap = conn.generate_id()?;
        conn.create_colormap(ColormapAlloc::NONE, colormap, root, visual)?;

        let win = conn.generate_id()?;
        let win_aux = CreateWindowAux::new()
            .background_pixel(0)
            .border_pixel(0)
            .override_redirect(1)
            .colormap(colormap)
            .event_mask(EventMask::STRUCTURE_NOTIFY);

        conn.create_window(
            32,
            win,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &win_aux,
        )?;

        // Set topmost / notification / Gamescope external overlay atoms
        if let (Some(wm_state), Some(above)) = (
            conn.intern_atom(false, b"_NET_WM_STATE")
                .ok()
                .and_then(|c| c.reply().ok()),
            conn.intern_atom(false, b"_NET_WM_STATE_ABOVE")
                .ok()
                .and_then(|c| c.reply().ok()),
        ) {
            let _ = conn.change_property(
                PropMode::REPLACE,
                win,
                wm_state.atom,
                AtomEnum::ATOM,
                32,
                1,
                &above.atom.to_ne_bytes(),
            );
        }

        if let Some(gs_atom) = conn
            .intern_atom(false, b"GAMESCOPE_EXTERNAL_OVERLAY")
            .ok()
            .and_then(|c| c.reply().ok())
        {
            let one: u32 = 1;
            let _ = conn.change_property(
                PropMode::REPLACE,
                win,
                gs_atom.atom,
                AtomEnum::CARDINAL,
                32,
                1,
                &one.to_ne_bytes(),
            );
        }

        // Empty XShape Input region so 100% of mouse clicks pass through to Star Citizen
        if conn
            .extension_information(shape::X11_EXTENSION_NAME)?
            .is_some()
        {
            let _ = conn.shape_rectangles(
                shape::SO::SET,
                shape::SK::INPUT,
                x11rb::protocol::xproto::ClipOrdering::UNSORTED,
                win,
                0,
                0,
                &[],
            );
        }

        let gc = conn.generate_id()?;
        conn.create_gc(gc, win, &CreateGCAux::new())?;
        conn.flush()?;

        Ok(Self {
            conn,
            win,
            gc,
            cur_bounds: IntRect::new(0, 0, 1, 1),
            mapped: false,
        })
    }

    fn present(&mut self, bounds: IntRect, bgra_premul: &[u8]) -> anyhow::Result<()> {
        if bounds.width <= 0 || bounds.height <= 0 {
            self.hide();
            return Ok(());
        }
        let w = bounds.width as usize;
        let h = bounds.height as usize;
        if bgra_premul.len() < w * h * 4 {
            return Ok(());
        }

        if bounds != self.cur_bounds {
            let cfg = ConfigureWindowAux::new()
                .x(bounds.x)
                .y(bounds.y)
                .width(bounds.width as u32)
                .height(bounds.height as u32)
                .stack_mode(StackMode::ABOVE);
            self.conn.configure_window(self.win, &cfg)?;
            // Re-assert empty input shape after resize
            let _ = self.conn.shape_rectangles(
                shape::SO::SET,
                shape::SK::INPUT,
                x11rb::protocol::xproto::ClipOrdering::UNSORTED,
                self.win,
                0,
                0,
                &[],
            );
            self.cur_bounds = bounds;
        }

        if !self.mapped {
            self.conn.map_window(self.win)?;
            self.mapped = true;
        }

        // Upload rows in chunks that stay well below X11 max_request_length
        let row_bytes = w * 4;
        let max_chunk_bytes = 256 * 1024;
        let rows_per_chunk = (max_chunk_bytes / row_bytes.max(1)).max(1).min(h);
        let mut y = 0usize;
        while y < h {
            let rows = rows_per_chunk.min(h - y);
            let slice = &bgra_premul[y * row_bytes..(y + rows) * row_bytes];
            self.conn.put_image(
                ImageFormat::Z_PIXMAP,
                self.win,
                self.gc,
                w as u16,
                rows as u16,
                0,
                y as i16,
                0,
                32,
                slice,
            )?;
            y += rows;
        }

        let cfg = ConfigureWindowAux::new().stack_mode(StackMode::ABOVE);
        let _ = self.conn.configure_window(self.win, &cfg);
        self.conn.flush()?;
        Ok(())
    }

    fn hide(&mut self) {
        if self.mapped {
            let _ = self.conn.unmap_window(self.win);
            let _ = self.conn.flush();
            self.mapped = false;
        }
    }
}

impl Drop for X11ArgbOverlay {
    fn drop(&mut self) {
        let _ = self.conn.free_gc(self.gc);
        let _ = self.conn.destroy_window(self.win);
        let _ = self.conn.flush();
    }
}

enum LinuxOverlayImpl {
    Wayland(WaylandLayerOverlay),
    X11(X11ArgbOverlay),
}

pub struct LinuxOverlayWindow {
    inner: LinuxOverlayImpl,
}

impl LinuxOverlayWindow {
    pub fn new() -> anyhow::Result<Self> {
        let is_gamescope = std::env::var_os("GAMESCOPE_WAYLAND_DISPLAY").is_some();
        if !is_gamescope && std::env::var_os("WAYLAND_DISPLAY").is_some() {
            match WaylandLayerOverlay::try_new() {
                Ok(wl) => {
                    tracing::info!("Initialized Wayland zwlr_layer_shell_v1 click-through overlay");
                    return Ok(Self {
                        inner: LinuxOverlayImpl::Wayland(wl),
                    });
                }
                Err(e) => {
                    tracing::info!(
                        "Wayland layer-shell unavailable ({}), falling back to X11/XWayland ARGB overlay",
                        e
                    );
                }
            }
        }

        let x11 = X11ArgbOverlay::try_new()?;
        tracing::info!("Initialized X11/XWayland/Gamescope 32-bit ARGB click-through overlay");
        Ok(Self {
            inner: LinuxOverlayImpl::X11(x11),
        })
    }
}

impl OverlayWindowBackend for LinuxOverlayWindow {
    fn present(
        &mut self,
        bounds: IntRect,
        bgra_premul: &[u8],
        _visible_in_capture: bool,
    ) -> anyhow::Result<()> {
        match &mut self.inner {
            LinuxOverlayImpl::Wayland(wl) => wl.present(bounds, bgra_premul),
            LinuxOverlayImpl::X11(x11) => x11.present(bounds, bgra_premul),
        }
    }

    fn hide(&mut self) {
        match &mut self.inner {
            LinuxOverlayImpl::Wayland(wl) => wl.hide(),
            LinuxOverlayImpl::X11(x11) => x11.hide(),
        }
    }
}
