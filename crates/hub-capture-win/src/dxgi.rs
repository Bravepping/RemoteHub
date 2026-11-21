use crate::{pack_rows, CaptureError, CapturedFrame};
use std::time::Instant;
use windows::{
    core::Interface,
    Win32::{
        Foundation::{HMODULE, POINT},
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
            Direct3D11::*,
            Dxgi::{Common::*, *},
            Gdi::{MonitorFromPoint, MONITOR_DEFAULTTOPRIMARY},
        },
    },
};

fn native_error(error: windows::core::Error) -> CaptureError {
    if error.code() == DXGI_ERROR_ACCESS_LOST {
        CaptureError::AccessLost
    } else {
        CaptureError::Unavailable(format!(
            "{} (HRESULT {:#010x})",
            error.message(),
            error.code().0 as u32
        ))
    }
}

pub struct DxgiCapture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    staging: Option<ID3D11Texture2D>,
    width: u32,
    height: u32,
    started: Instant,
    adapter_name: String,
}

// Releases the acquired duplication frame on every return/error path.
struct AcquiredFrame<'a>(&'a IDXGIOutputDuplication);
impl Drop for AcquiredFrame<'_> {
    fn drop(&mut self) {
        unsafe {
            let _ = self.0.ReleaseFrame();
        }
    }
}

struct MappedTexture<'a>(&'a ID3D11DeviceContext, &'a ID3D11Texture2D);
impl Drop for MappedTexture<'_> {
    fn drop(&mut self) {
        unsafe {
            self.0.Unmap(self.1, 0);
        }
    }
}

impl DxgiCapture {
    pub fn new() -> Result<Self, CaptureError> {
        // All COM objects are owned by this capture instance. No raw interface pointer escapes.
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().map_err(native_error)?;
            let primary = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let mut adapter_index = 0;
            loop {
                let adapter = match factory.EnumAdapters1(adapter_index) {
                    Ok(adapter) => adapter,
                    Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                    Err(e) => return Err(native_error(e)),
                };
                adapter_index += 1;
                let mut output_index = 0;
                loop {
                    let output = match adapter.EnumOutputs(output_index) {
                        Ok(output) => output,
                        Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                        Err(e) => return Err(native_error(e)),
                    };
                    output_index += 1;
                    let description = output.GetDesc().map_err(native_error)?;
                    if description.Monitor != primary || !description.AttachedToDesktop.as_bool() {
                        continue;
                    }
                    if description.Rotation != DXGI_MODE_ROTATION_IDENTITY {
                        return Err(CaptureError::Unavailable(
                            "首轮实验尚不支持旋转显示器".into(),
                        ));
                    }
                    let base_adapter: IDXGIAdapter = adapter.cast().map_err(native_error)?;
                    let mut device = None;
                    let mut context = None;
                    D3D11CreateDevice(
                        &base_adapter,
                        D3D_DRIVER_TYPE_UNKNOWN,
                        HMODULE::default(),
                        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                        None,
                        D3D11_SDK_VERSION,
                        Some(&mut device),
                        None,
                        Some(&mut context),
                    )
                    .map_err(native_error)?;
                    let device = device
                        .ok_or_else(|| CaptureError::Unavailable("D3D11 device missing".into()))?;
                    let context = context
                        .ok_or_else(|| CaptureError::Unavailable("D3D11 context missing".into()))?;
                    let output1: IDXGIOutput1 = output.cast().map_err(native_error)?;
                    let duplication = output1.DuplicateOutput(&device).map_err(native_error)?;
                    let adapter_desc = adapter.GetDesc1().map_err(native_error)?;
                    let end = adapter_desc
                        .Description
                        .iter()
                        .position(|c| *c == 0)
                        .unwrap_or(adapter_desc.Description.len());
                    return Ok(Self {
                        device,
                        context,
                        duplication,
                        staging: None,
                        width: 0,
                        height: 0,
                        started: Instant::now(),
                        adapter_name: String::from_utf16_lossy(&adapter_desc.Description[..end]),
                    });
                }
            }
        }
        Err(CaptureError::Unavailable(
            "没有可用的主显示器 DXGI output".into(),
        ))
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    pub fn next_frame(&mut self, timeout_ms: u32) -> Result<Option<CapturedFrame>, CaptureError> {
        if timeout_ms > 1000 {
            return Err(CaptureError::InvalidArgument("timeout must be <= 1000ms"));
        }
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource = None;
            match self
                .duplication
                .AcquireNextFrame(timeout_ms, &mut info, &mut resource)
            {
                Ok(()) => (),
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
                Err(e) => return Err(native_error(e)),
            }
            let _acquired = AcquiredFrame(&self.duplication);
            let resource = resource
                .ok_or_else(|| CaptureError::Unavailable("DXGI frame resource missing".into()))?;
            let texture: ID3D11Texture2D = resource.cast().map_err(native_error)?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
                return Err(CaptureError::Unavailable("DXGI frame is not BGRA8".into()));
            }
            if self.staging.is_none() || self.width != desc.Width || self.height != desc.Height {
                desc.Usage = D3D11_USAGE_STAGING;
                desc.BindFlags = 0;
                desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                desc.MiscFlags = 0;
                let mut staging = None;
                self.device
                    .CreateTexture2D(&desc, None, Some(&mut staging))
                    .map_err(native_error)?;
                self.staging = staging;
                self.width = desc.Width;
                self.height = desc.Height;
            }
            let staging = self
                .staging
                .as_ref()
                .ok_or_else(|| CaptureError::Unavailable("staging texture missing".into()))?;
            self.context.CopyResource(staging, &texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(native_error)?;
            let _mapped = MappedTexture(&self.context, staging);
            if mapped.pData.is_null()
                || self.height == 0
                || self.height > 16384
                || self.width > 16384
            {
                return Err(CaptureError::Unavailable(
                    "invalid mapped DXGI frame".into(),
                ));
            }
            let size = (mapped.RowPitch as usize)
                .checked_mul(self.height as usize)
                .ok_or(CaptureError::InvalidArgument("mapped buffer overflow"))?;
            // D3D11 guarantees RowPitch bytes for each row of this mapped 2D staging texture.
            let data = std::slice::from_raw_parts(mapped.pData.cast::<u8>(), size);
            let bgra = pack_rows(data, self.width, self.height, mapped.RowPitch as usize)?;
            Ok(Some(CapturedFrame {
                width: self.width,
                height: self.height,
                timestamp_us: self.started.elapsed().as_micros() as u64,
                bgra,
            }))
        }
    }
}
