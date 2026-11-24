use crate::{
    surface::{PixelLayout, SurfaceKey},
    CaptureError, CapturedFrame,
};
use std::time::Instant;
use windows::{
    core::Interface,
    Win32::{
        Foundation::{E_NOINTERFACE, E_NOTIMPL, HMODULE},
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
            Direct3D11::*,
            Dxgi::{Common::*, *},
            Gdi::{GetMonitorInfoW, MONITORINFO},
        },
        UI::WindowsAndMessaging::MONITORINFOF_PRIMARY,
    },
};

fn native_error(stage: &'static str, error: windows::core::Error) -> CaptureError {
    if error.code() == DXGI_ERROR_ACCESS_LOST {
        CaptureError::AccessLost
    } else {
        CaptureError::Dxgi {
            stage,
            code: error.code().0 as u32,
        }
    }
}

fn duplicate_output(
    output: &IDXGIOutput,
    device: &ID3D11Device,
) -> Result<IDXGIOutputDuplication, CaptureError> {
    // Request a single SDR scan-out format. DXGI may convert to this format
    // rather than returning the original fullscreen/HDR render target.
    // Fall back only if the API itself is unavailable, not if Windows
    // refuses this output, desktop mode, session, or allocation.
    match output.cast::<IDXGIOutput5>() {
        Ok(output5) => {
            // SAFETY: device was created on this exact output's DXGI adapter.
            match unsafe { output5.DuplicateOutput1(device, 0, &[DXGI_FORMAT_B8G8R8A8_UNORM]) } {
                Ok(duplication) => return Ok(duplication),
                Err(error) if error.code() == E_NOTIMPL => (),
                Err(error) => return Err(native_error("DUPLICATE_OUTPUT", error)),
            }
        }
        Err(error) if error.code() == E_NOINTERFACE => (),
        Err(error) => return Err(native_error("OUTPUT_CAST", error)),
    }
    let output1: IDXGIOutput1 = output.cast().map_err(|e| native_error("OUTPUT_CAST", e))?;
    // SAFETY: same-adapter binding as above, using the original interface.
    unsafe { output1.DuplicateOutput(device) }.map_err(|e| native_error("DUPLICATE_OUTPUT", e))
}

pub struct DxgiCapture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    staging: Option<(SurfaceKey, ID3D11Texture2D)>,
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
            let factory: IDXGIFactory1 =
                CreateDXGIFactory1().map_err(|e| native_error("CREATE_FACTORY", e))?;
            let mut adapter_index = 0;
            loop {
                let adapter = match factory.EnumAdapters1(adapter_index) {
                    Ok(adapter) => adapter,
                    Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                    Err(e) => return Err(native_error("ENUM_ADAPTERS", e)),
                };
                adapter_index += 1;
                let mut output_index = 0;
                loop {
                    let output = match adapter.EnumOutputs(output_index) {
                        Ok(output) => output,
                        Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                        Err(e) => return Err(native_error("ENUM_OUTPUTS", e)),
                    };
                    output_index += 1;
                    let description = output
                        .GetDesc()
                        .map_err(|e| native_error("OUTPUT_DESC", e))?;
                    if !description.AttachedToDesktop.as_bool() || description.Monitor.is_invalid()
                    {
                        continue;
                    }
                    // Use Windows' actual primary flag, not a point in a
                    // potentially virtualized DPI coordinate space.
                    let mut info = MONITORINFO {
                        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                        ..Default::default()
                    };
                    if !GetMonitorInfoW(description.Monitor, &mut info).as_bool()
                        || info.dwFlags & MONITORINFOF_PRIMARY == 0
                    {
                        continue;
                    }
                    if description.Rotation != DXGI_MODE_ROTATION_IDENTITY {
                        return Err(CaptureError::DxgiState("ROTATED_PRIMARY"));
                    }
                    let base_adapter: IDXGIAdapter = adapter
                        .cast()
                        .map_err(|e| native_error("ADAPTER_CAST", e))?;
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
                    .map_err(|e| native_error("CREATE_DEVICE", e))?;
                    let device = device.ok_or(CaptureError::DxgiState("DEVICE_MISSING"))?;
                    let context = context.ok_or(CaptureError::DxgiState("CONTEXT_MISSING"))?;
                    let duplication = duplicate_output(&output, &device)?;
                    let adapter_desc = adapter
                        .GetDesc1()
                        .map_err(|e| native_error("ADAPTER_DESC", e))?;
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
                        started: Instant::now(),
                        adapter_name: String::from_utf16_lossy(&adapter_desc.Description[..end]),
                    });
                }
            }
        }
        Err(CaptureError::DxgiState("NO_ATTACHED_PRIMARY_OUTPUT"))
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
                Err(e) => return Err(native_error("ACQUIRE_FRAME", e)),
            }
            let _acquired = AcquiredFrame(&self.duplication);
            let resource = resource.ok_or(CaptureError::DxgiState("FRAME_RESOURCE_MISSING"))?;
            let texture: ID3D11Texture2D = resource
                .cast()
                .map_err(|e| native_error("TEXTURE_CAST", e))?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            let key = SurfaceKey {
                width: desc.Width,
                height: desc.Height,
                format: desc.Format.0 as u32,
            };
            let layout = PixelLayout::classify(key.format)?;
            layout.row_bytes(key.width, key.height)?;
            if !self
                .staging
                .as_ref()
                .is_some_and(|(cached, _)| *cached == key)
            {
                desc.Usage = D3D11_USAGE_STAGING;
                desc.BindFlags = 0;
                desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                desc.MiscFlags = 0;
                let mut staging = None;
                self.device
                    .CreateTexture2D(&desc, None, Some(&mut staging))
                    .map_err(|e| native_error("CREATE_STAGING", e))?;
                self.staging = staging.map(|staging| (key, staging));
            }
            let (_, staging) = self
                .staging
                .as_ref()
                .ok_or(CaptureError::DxgiState("STAGING_MISSING"))?;
            self.context.CopyResource(staging, &texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|e| native_error("MAP_STAGING", e))?;
            let _mapped = MappedTexture(&self.context, staging);
            if mapped.pData.is_null() {
                return Err(CaptureError::DxgiState("FRAME_GEOMETRY"));
            }
            let size = layout.mapped_len(key.width, key.height, mapped.RowPitch as usize)?;
            // Borrow the mapped surface only while mapped. The FP16 decoder
            // accesses the real pixel bytes one row at a time, not padding.
            let data = std::slice::from_raw_parts(mapped.pData.cast::<u8>(), size);
            let bgra = layout.pack_bgra(data, key.width, key.height, mapped.RowPitch as usize)?;
            Ok(Some(CapturedFrame {
                width: key.width,
                height: key.height,
                timestamp_us: self.started.elapsed().as_micros() as u64,
                bgra,
            }))
        }
    }
}
