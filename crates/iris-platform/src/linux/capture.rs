use crate::{FrameCapturer, GameWindowInfo};
use ashpd::desktop::screencast::{CursorMode, Screencast, SourceType};
use ashpd::desktop::PersistMode;
use iris_core::config::AppSettings;
use iris_core::models::IntRect;
use iris_core::pipeline::CapturedFrame;
use parking_lot::Mutex;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, ImageFormat, Window};
use x11rb::rust_connection::RustConnection;

struct LatestRawBgra {
    width: usize,
    height: usize,
    data: Vec<u8>,
    seq: u64,
}

struct PipeWireGstPipeline {
    child: Child,
    latest: Arc<Mutex<Option<LatestRawBgra>>>,
    last_seq: u64,
}

impl Drop for PipeWireGstPipeline {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct LinuxFrameCapturer {
    pw_pipeline: Option<PipeWireGstPipeline>,
    pw_failed_at: Option<Instant>,
    x11: Option<(RustConnection, usize)>,
}

impl LinuxFrameCapturer {
    pub fn new() -> Self {
        Self {
            pw_pipeline: None,
            pw_failed_at: None,
            x11: None,
        }
    }

    fn start_portal_pipewire(
        width_hint: i32,
        height_hint: i32,
        restore_token: &str,
    ) -> anyhow::Result<(PipeWireGstPipeline, Option<String>)> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        let prev_tok = if restore_token.trim().is_empty() {
            None
        } else {
            Some(restore_token.trim())
        };

        let (raw_dup_fd, node_id, stream_w, stream_h, new_token) =
            rt.block_on(async move {
                let proxy = Screencast::new().await?;
                let session = proxy.create_session().await?;
                proxy
                    .select_sources(
                        &session,
                        CursorMode::Hidden,
                        SourceType::Monitor | SourceType::Window,
                        false,
                        prev_tok,
                        PersistMode::ExplicitlyRevoked,
                    )
                    .await?;

                let response = proxy.start(&session, None).await?.response()?;
                let new_tok = response.restore_token().map(|s| s.to_string());
                let stream = response
                    .streams()
                    .first()
                    .ok_or_else(|| anyhow::anyhow!("XDG ScreenCast returned zero streams"))?;
                let node_id = stream.pipe_wire_node_id();
                let (sw, sh) = stream
                    .size()
                    .unwrap_or((width_hint.max(1280), height_hint.max(720)));

                let pw_fd = proxy.open_pipe_wire_remote(&session).await?;
                // Duplicate FD without FD_CLOEXEC so child `gst-launch-1.0` inherits it
                let dup_fd = unsafe { libc::dup(pw_fd.as_raw_fd()) };
                if dup_fd < 0 {
                    anyhow::bail!("dup() failed on PipeWire remote fd");
                }
                unsafe {
                    libc::fcntl(dup_fd, libc::F_SETFD, 0);
                }

                Ok::<_, anyhow::Error>((
                    dup_fd,
                    node_id,
                    sw.max(64) as usize,
                    sh.max(64) as usize,
                    new_tok,
                ))
            })?;

        let caps = format!(
            "video/x-raw,format=BGRA,width={},height={}",
            stream_w, stream_h
        );
        let child_res = Command::new("gst-launch-1.0")
            .args([
                "-q",
                "pipewiresrc",
                &format!("fd={}", raw_dup_fd),
                &format!("path={}", node_id),
                "do-timestamp=true",
                "!",
                "videoconvert",
                "!",
                "videoscale",
                "!",
                &caps,
                "!",
                "fdsink",
                "fd=1",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();

        // Close parent's copy of the duplicated inheritable FD now that child has spawned
        unsafe {
            libc::close(raw_dup_fd);
        }

        let mut child = child_res?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture gst-launch-1.0 stdout"))?;

        let latest: Arc<Mutex<Option<LatestRawBgra>>> = Arc::new(Mutex::new(None));
        let latest_writer = Arc::clone(&latest);
        let frame_bytes = stream_w * stream_h * 4;

        thread::Builder::new()
            .name("sc-overlay-pw-reader".to_string())
            .spawn(move || {
                let mut seq = 0u64;
                loop {
                    let mut buf = vec![0u8; frame_bytes];
                    if stdout.read_exact(&mut buf).is_err() {
                        break;
                    }
                    seq += 1;
                    *latest_writer.lock() = Some(LatestRawBgra {
                        width: stream_w,
                        height: stream_h,
                        data: buf,
                        seq,
                    });
                }
            })?;

        Ok((
            PipeWireGstPipeline {
                child,
                latest,
                last_seq: 0,
            },
            new_token,
        ))
    }

    fn capture_x11_fallback(
        &mut self,
        game: &GameWindowInfo,
        now_ms: f64,
    ) -> anyhow::Result<Option<CapturedFrame>> {
        if self.x11.is_none() {
            self.x11 = Some(x11rb::connect(None)?);
        }
        let (conn, screen_num) = self.x11.as_ref().unwrap();
        let screen = conn
            .setup()
            .roots
            .get(*screen_num)
            .ok_or_else(|| anyhow::anyhow!("Invalid X11 screen"))?;

        let (drawable, x, y, w, h) = if game.native_handle != 0
            && game.bounds.width >= 64
            && game.bounds.height >= 64
        {
            (
                game.native_handle as Window,
                0i16,
                0i16,
                game.bounds.width as u16,
                game.bounds.height as u16,
            )
        } else {
            let r: IntRect = if game.bounds.is_empty() {
                IntRect::new(
                    0,
                    0,
                    screen.width_in_pixels as i32,
                    screen.height_in_pixels as i32,
                )
            } else {
                game.bounds.clamp_to(
                    screen.width_in_pixels as i32,
                    screen.height_in_pixels as i32,
                )
            };
            if r.width < 64 || r.height < 64 {
                return Ok(None);
            }
            (
                screen.root,
                r.x as i16,
                r.y as i16,
                r.width as u16,
                r.height as u16,
            )
        };

        let reply = conn
            .get_image(ImageFormat::Z_PIXMAP, drawable, x, y, w, h, !0)
            .map_err(|e| anyhow::anyhow!("X11 get_image failed: {}", e))?
            .reply()
            .map_err(|e| anyhow::anyhow!("X11 get_image reply failed: {}", e))?;

        let width = w as usize;
        let height = h as usize;
        if reply.data.len() < width * height * 4 {
            return Ok(None);
        }

        Ok(Some(CapturedFrame::from_bgra8(
            width,
            height,
            width * 4,
            &reply.data,
            now_ms,
        )))
    }
}

impl Default for LinuxFrameCapturer {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameCapturer for LinuxFrameCapturer {
    fn capture_frame(
        &mut self,
        game: &GameWindowInfo,
        settings: &mut AppSettings,
        now_ms: f64,
    ) -> anyhow::Result<(Option<CapturedFrame>, bool)> {
        let mut settings_changed = false;
        let is_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();

        if is_wayland {
            let backoff_expired = self
                .pw_failed_at
                .map(|t| t.elapsed() >= Duration::from_secs(10))
                .unwrap_or(true);

            if self.pw_pipeline.is_none() && backoff_expired {
                match Self::start_portal_pipewire(
                    game.bounds.width,
                    game.bounds.height,
                    &settings.linux_portal_restore_token,
                ) {
                    Ok((pipe, maybe_tok)) => {
                        if let Some(tok) = maybe_tok {
                            if !tok.is_empty() && tok != settings.linux_portal_restore_token {
                                settings.linux_portal_restore_token = tok;
                                settings_changed = true;
                            }
                        }
                        self.pw_pipeline = Some(pipe);
                        self.pw_failed_at = None;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Wayland ScreenCast portal unavailable ({}), trying X11/XWayland fallback",
                            e
                        );
                        self.pw_failed_at = Some(Instant::now());
                    }
                }
            }

            if let Some(pipe) = &mut self.pw_pipeline {
                // Check if gst-launch exited unexpectedly
                if let Ok(Some(_status)) = pipe.child.try_wait() {
                    self.pw_pipeline = None;
                    self.pw_failed_at = Some(Instant::now());
                } else {
                    let guard = pipe.latest.lock();
                    if let Some(raw) = guard.as_ref() {
                        if raw.seq != pipe.last_seq {
                            pipe.last_seq = raw.seq;
                            let frame = CapturedFrame::from_bgra8(
                                raw.width,
                                raw.height,
                                raw.width * 4,
                                &raw.data,
                                now_ms,
                            );
                            return Ok((Some(frame), settings_changed));
                        }
                    }
                    return Ok((None, settings_changed));
                }
            }
        }

        // X11 / XWayland / Gamescope fallback
        let frame = self.capture_x11_fallback(game, now_ms)?;
        Ok((frame, settings_changed))
    }

    fn reset(&mut self) {
        self.pw_pipeline = None;
        self.pw_failed_at = None;
        self.x11 = None;
    }
}
