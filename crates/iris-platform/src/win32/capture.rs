use crate::{FrameCapturer, GameWindowInfo};
use iris_core::config::{AppSettings, CaptureMethod};
use iris_core::models::IntRect;
use iris_core::pipeline::CapturedFrame;
use windows::core::Interface;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_STAGING, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
    IDXGIDevice, IDXGIFactory1, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

struct StagingTexture {
    tex: ID3D11Texture2D,
    width: u32,
    height: u32,
}

struct DxgiDuplicator {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    output_bounds: IntRect,
    staging: Option<StagingTexture>,
}

struct WinRtWindowCapture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    d3d_winrt: IDirect3DDevice,
    hwnd: u64,
    size: SizeInt32,
    hide_border: bool,
    frame_pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    staging: Option<StagingTexture>,
}

impl Drop for WinRtWindowCapture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.frame_pool.Close();
    }
}

pub struct Win32FrameCapturer {
    duplicator: Option<DxgiDuplicator>,
    winrt: Option<WinRtWindowCapture>,
    last_method: Option<CaptureMethod>,
}

// D3D11 free-threaded device & WinRT Direct3D11CaptureFramePool::CreateFreeThreaded are agile across threads
unsafe impl Send for Win32FrameCapturer {}

impl Win32FrameCapturer {
    pub fn new() -> Self {
        Self {
            duplicator: None,
            winrt: None,
            last_method: None,
        }
    }

    fn create_d3d11_device() -> anyhow::Result<(ID3D11Device, ID3D11DeviceContext)> {
        unsafe {
            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;
            let feature_levels = [D3D_FEATURE_LEVEL_11_0];
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                None,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            Ok((
                device.ok_or_else(|| anyhow::anyhow!("D3D11CreateDevice returned null device"))?,
                context.ok_or_else(|| anyhow::anyhow!("D3D11CreateDevice returned null context"))?,
            ))
        }
    }

    fn ensure_staging(
        device: &ID3D11Device,
        slot: &mut Option<StagingTexture>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<ID3D11Texture2D> {
        if let Some(existing) = slot {
            if existing.width == width && existing.height == height {
                return Ok(existing.tex.clone());
            }
        }
        unsafe {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut tex: Option<ID3D11Texture2D> = None;
            device.CreateTexture2D(&desc, None, Some(&mut tex))?;
            let tex = tex.ok_or_else(|| anyhow::anyhow!("CreateTexture2D returned null"))?;
            *slot = Some(StagingTexture {
                tex: tex.clone(),
                width,
                height,
            });
            Ok(tex)
        }
    }

    fn map_and_read(
        context: &ID3D11DeviceContext,
        staging: &ID3D11Texture2D,
        width: usize,
        height: usize,
        now_ms: f64,
    ) -> anyhow::Result<CapturedFrame> {
        unsafe {
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let stride = mapped.RowPitch as usize;
            let total_bytes = stride * height;
            let slice = std::slice::from_raw_parts(mapped.pData as *const u8, total_bytes);
            let frame = CapturedFrame::from_bgra8(width, height, stride, slice, now_ms);
            context.Unmap(staging, 0);
            Ok(frame)
        }
    }

    fn init_duplicator(target_bounds: IntRect) -> anyhow::Result<DxgiDuplicator> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let mut best_adapter = None;
            let mut best_output = None;
            let mut best_rect = IntRect::default();
            let mut best_area = -1i64;

            let mut adapter_idx = 0u32;
            while let Ok(adapter) = factory.EnumAdapters1(adapter_idx) {
                adapter_idx += 1;
                let mut output_idx = 0u32;
                while let Ok(output) = adapter.EnumOutputs(output_idx) {
                    output_idx += 1;
                    if let Ok(desc) = output.GetDesc() {
                        let r = desc.DesktopCoordinates;
                        let out_rect =
                            IntRect::new(r.left, r.top, r.right - r.left, r.bottom - r.top);
                        let ix0 = target_bounds.x.max(out_rect.x);
                        let iy0 = target_bounds.y.max(out_rect.y);
                        let ix1 = target_bounds.right().min(out_rect.right());
                        let iy1 = target_bounds.bottom().min(out_rect.bottom());
                        let area =
                            ((ix1 - ix0).max(0) as i64) * ((iy1 - iy0).max(0) as i64);
                        if area > best_area {
                            best_area = area;
                            best_adapter = Some(adapter.clone());
                            best_output = Some(output);
                            best_rect = out_rect;
                        }
                    }
                }
            }

            let adapter =
                best_adapter.ok_or_else(|| anyhow::anyhow!("No DXGI adapter found"))?;
            let output = best_output.ok_or_else(|| anyhow::anyhow!("No DXGI output found"))?;
            let output1: IDXGIOutput1 = output.cast()?;

            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;
            let feature_levels = [D3D_FEATURE_LEVEL_11_0];
            D3D11CreateDevice(
                &adapter,
                windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
                None,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device = device.ok_or_else(|| anyhow::anyhow!("Null D3D11 device"))?;
            let context = context.ok_or_else(|| anyhow::anyhow!("Null D3D11 context"))?;
            let duplication = output1.DuplicateOutput(&device)?;

            Ok(DxgiDuplicator {
                device,
                context,
                duplication,
                output_bounds: best_rect,
                staging: None,
            })
        }
    }

    fn capture_desktop_duplication(
        &mut self,
        game: &GameWindowInfo,
        now_ms: f64,
    ) -> anyhow::Result<Option<CapturedFrame>> {
        let need_reinit = match &self.duplicator {
            None => true,
            Some(dup) => {
                let cx = game.bounds.x + game.bounds.width / 2;
                let cy = game.bounds.y + game.bounds.height / 2;
                cx < dup.output_bounds.x
                    || cx > dup.output_bounds.right()
                    || cy < dup.output_bounds.y
                    || cy > dup.output_bounds.bottom()
            }
        };
        if need_reinit {
            self.duplicator = Some(Self::init_duplicator(game.bounds)?);
        }

        let dup = self.duplicator.as_mut().unwrap();
        unsafe {
            let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource: Option<IDXGIResource> = None;
            match dup
                .duplication
                .AcquireNextFrame(40, &mut frame_info, &mut resource)
            {
                Ok(()) => {}
                Err(err) if err.code() == DXGI_ERROR_WAIT_TIMEOUT => {
                    return Ok(None);
                }
                Err(err) if err.code() == DXGI_ERROR_ACCESS_LOST => {
                    self.duplicator = None;
                    return Err(err.into());
                }
                Err(err) => {
                    self.duplicator = None;
                    return Err(err.into());
                }
            }

            let copy_result = (|| -> anyhow::Result<Option<CapturedFrame>> {
                let Some(res) = resource else {
                    return Ok(None);
                };
                let src_tex: ID3D11Texture2D = res.cast()?;
                let mut src_desc = D3D11_TEXTURE2D_DESC::default();
                src_tex.GetDesc(&mut src_desc);

                let local_x = (game.bounds.x - dup.output_bounds.x).max(0) as u32;
                let local_y = (game.bounds.y - dup.output_bounds.y).max(0) as u32;
                let max_w = src_desc.Width.saturating_sub(local_x);
                let max_h = src_desc.Height.saturating_sub(local_y);
                let copy_w = (game.bounds.width.max(0) as u32).min(max_w);
                let copy_h = (game.bounds.height.max(0) as u32).min(max_h);
                if copy_w < 64 || copy_h < 64 {
                    return Ok(None);
                }

                let staging =
                    Self::ensure_staging(&dup.device, &mut dup.staging, copy_w, copy_h)?;
                let src_box = D3D11_BOX {
                    left: local_x,
                    top: local_y,
                    front: 0,
                    right: local_x + copy_w,
                    bottom: local_y + copy_h,
                    back: 1,
                };
                dup.context
                    .CopySubresourceRegion(&staging, 0, 0, 0, 0, &src_tex, 0, Some(&src_box));
                let frame = Self::map_and_read(
                    &dup.context,
                    &staging,
                    copy_w as usize,
                    copy_h as usize,
                    now_ms,
                )?;
                Ok(Some(frame))
            })();

            let _ = dup.duplication.ReleaseFrame();
            copy_result
        }
    }

    fn init_winrt_capture(
        hwnd_u64: u64,
        width: i32,
        height: i32,
        hide_border: bool,
    ) -> anyhow::Result<WinRtWindowCapture> {
        unsafe {
            let (device, context) = Self::create_d3d11_device()?;
            let dxgi_device: IDXGIDevice = device.cast()?;
            let inspectable = CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device)?;
            let d3d_winrt: IDirect3DDevice = inspectable.cast()?;

            let interop: IGraphicsCaptureItemInterop =
                windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
            let hwnd = HWND(hwnd_u64 as usize as *mut _);
            let item: GraphicsCaptureItem = interop.CreateForWindow(hwnd)?;
            let item_size = item.Size().unwrap_or(SizeInt32 {
                Width: width.max(64),
                Height: height.max(64),
            });
            let size = SizeInt32 {
                Width: item_size.Width.max(64),
                Height: item_size.Height.max(64),
            };

            let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
                &d3d_winrt,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                2,
                size,
            )?;
            let session = frame_pool.CreateCaptureSession(&item)?;
            let _ = session.SetIsCursorCaptureEnabled(false);
            let _ = session.SetIsBorderRequired(!hide_border);
            session.StartCapture()?;

            Ok(WinRtWindowCapture {
                device,
                context,
                d3d_winrt,
                hwnd: hwnd_u64,
                size,
                hide_border,
                frame_pool,
                session,
                staging: None,
            })
        }
    }

    fn capture_winrt_window(
        &mut self,
        game: &GameWindowInfo,
        hide_border: bool,
        now_ms: f64,
    ) -> anyhow::Result<Option<CapturedFrame>> {
        if game.native_handle == 0 {
            return Ok(None);
        }
        let need_reinit = match &self.winrt {
            None => true,
            Some(w) => w.hwnd != game.native_handle || w.hide_border != hide_border,
        };
        if need_reinit {
            self.winrt = Some(Self::init_winrt_capture(
                game.native_handle,
                game.bounds.width,
                game.bounds.height,
                hide_border,
            )?);
        }

        let w = self.winrt.as_mut().unwrap();
        let Ok(frame) = w.frame_pool.TryGetNextFrame() else {
            return Ok(None);
        };

        let result = unsafe {
            let content_size = frame.ContentSize()?;
            if content_size.Width > 0
                && content_size.Height > 0
                && (content_size.Width != w.size.Width || content_size.Height != w.size.Height)
            {
                w.size = content_size;
                let _ = w.frame_pool.Recreate(
                    &w.d3d_winrt,
                    DirectXPixelFormat::B8G8R8A8UIntNormalized,
                    2,
                    w.size,
                );
            }

            let surface = frame.Surface()?;
            let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
            let src_tex: ID3D11Texture2D = access.GetInterface()?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            src_tex.GetDesc(&mut desc);
            if desc.Width < 64 || desc.Height < 64 {
                Ok(None)
            } else {
                let staging =
                    Self::ensure_staging(&w.device, &mut w.staging, desc.Width, desc.Height)?;
                w.context.CopyResource(&staging, &src_tex);
                let captured = Self::map_and_read(
                    &w.context,
                    &staging,
                    desc.Width as usize,
                    desc.Height as usize,
                    now_ms,
                )?;
                Ok(Some(captured))
            }
        };

        let _ = frame.Close();
        result
    }
}

impl Default for Win32FrameCapturer {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameCapturer for Win32FrameCapturer {
    fn capture_frame(
        &mut self,
        game: &GameWindowInfo,
        settings: &mut AppSettings,
        now_ms: f64,
    ) -> anyhow::Result<(Option<CapturedFrame>, bool)> {
        if self.last_method != Some(settings.capture_method) {
            self.reset();
            self.last_method = Some(settings.capture_method);
        }

        let frame = match settings.capture_method {
            CaptureMethod::DesktopDuplication => self.capture_desktop_duplication(game, now_ms)?,
            CaptureMethod::WindowCapture => {
                match self.capture_winrt_window(game, settings.hide_capture_border, now_ms) {
                    Ok(f) => f,
                    Err(_) => self.capture_desktop_duplication(game, now_ms)?,
                }
            }
        };
        Ok((frame, false))
    }

    fn reset(&mut self) {
        self.duplicator = None;
        self.winrt = None;
    }
}
