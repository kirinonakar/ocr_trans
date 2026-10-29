//! A persistent, worker-owned WGC session. It captures the selected HWND even
//! when the magnifier covers it; desktop readback would recurse into the viewer.
use anyhow::{Context, Result};
use image::RgbaImage;
use windows::{
    core::{factory, Interface},
    Graphics::{
        Capture::{
            Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem,
            GraphicsCaptureSession,
        },
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
        SizeInt32,
    },
    Win32::{
        Foundation::{HMODULE, HWND},
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP},
            Direct3D11::{
                D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource,
                ID3D11Texture2D, D3D11_BOX, D3D11_CPU_ACCESS_READ,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ,
                D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
            },
            Dxgi::IDXGIDevice,
        },
        System::WinRT::{
            Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess},
            Graphics::Capture::IGraphicsCaptureItemInterop,
            RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED,
        },
    },
};

pub(super) struct Apartment;
impl Apartment {
    pub fn new() -> Result<Self> {
        unsafe {
            RoInitialize(RO_INIT_MULTITHREADED)?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            RoUninitialize();
        }
    }
}

struct Frame(Direct3D11CaptureFrame);
impl Drop for Frame {
    fn drop(&mut self) {
        let _ = self.0.Close();
    }
}

pub(super) struct WindowCapture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt_device: IDirect3DDevice,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    size: SizeInt32,
    staging: Option<ID3D11Texture2D>,
}

impl Drop for WindowCapture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

impl WindowCapture {
    pub fn new(source: isize) -> Result<Self> {
        anyhow::ensure!(
            GraphicsCaptureSession::IsSupported()?,
            "Window graphics capture is unavailable"
        );
        let create_device = |driver| -> Result<ID3D11Device> {
            let mut device = None;
            unsafe {
                D3D11CreateDevice(
                    None,
                    driver,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    None,
                )?;
            }
            device.context("D3D11 device was not created")
        };
        let device = create_device(D3D_DRIVER_TYPE_HARDWARE)
            .or_else(|_| create_device(D3D_DRIVER_TYPE_WARP))?;
        let context = unsafe { device.GetImmediateContext()? };
        let dxgi: IDXGIDevice = device.cast()?;
        let winrt_device: IDirect3DDevice =
            unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? }.cast()?;
        Self::with_device(source, device, context, winrt_device)
    }

    pub fn related_window(&self, source: isize) -> Result<Self> {
        Self::with_device(
            source,
            self.device.clone(),
            self.context.clone(),
            self.winrt_device.clone(),
        )
    }

    fn with_device(
        source: isize,
        device: ID3D11Device,
        context: ID3D11DeviceContext,
        winrt_device: IDirect3DDevice,
    ) -> Result<Self> {
        let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(HWND(source as _))? };
        let size = item.Size()?;
        anyhow::ensure!(
            size.Width > 0 && size.Height > 0,
            "The selected window has no drawable surface"
        );
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )?;
        let session = pool.CreateCaptureSession(&item)?;
        let this = Self {
            device,
            context,
            winrt_device,
            pool,
            session,
            size,
            staging: None,
        };
        // Older Windows builds can reject these optional properties.
        let _ = this.session.SetIsCursorCaptureEnabled(false);
        let _ = this.session.SetIsBorderRequired(false);
        this.session.StartCapture()?;
        Ok(this)
    }

    pub fn next_frame(&mut self) -> Result<Option<RgbaImage>> {
        let mut frame = match self.pool.TryGetNextFrame() {
            Ok(frame) => Frame(frame),
            Err(_) => {
                // Empty frame pools are normal for static windows. Device loss isn't.
                unsafe {
                    self.device.GetDeviceRemovedReason()?;
                }
                return Ok(None);
            }
        };
        // Process the latest queued frame rather than building up video latency.
        for _ in 0..2 {
            match self.pool.TryGetNextFrame() {
                Ok(newest) => frame = Frame(newest),
                Err(_) => break,
            }
        }
        let size = frame.0.ContentSize()?;
        if size.Width <= 0 || size.Height <= 0 {
            return Ok(None);
        }
        if (size.Width, size.Height) != (self.size.Width, self.size.Height) {
            // Release the borrowed frame before recreating the capture buffers.
            drop(frame);
            self.pool.Recreate(
                &self.winrt_device,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                2,
                size,
            )?;
            self.size = size;
            self.staging = None;
            return Ok(None);
        }
        let surface = frame.0.Surface()?;
        let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
        let texture: ID3D11Texture2D = unsafe { access.GetInterface()? };
        unsafe { self.read_pixels(&texture).map(Some) }
    }

    unsafe fn read_pixels(&mut self, texture: &ID3D11Texture2D) -> Result<RgbaImage> {
        let width = self.size.Width as u32;
        let height = self.size.Height as u32;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        anyhow::ensure!(
            width <= desc.Width && height <= desc.Height,
            "Incomplete captured surface"
        );
        if self.staging.is_none() {
            desc.Width = width;
            desc.Height = height;
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.MiscFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut staging = None;
            self.device
                .CreateTexture2D(&desc, None, Some(&mut staging))?;
            self.staging = Some(staging.context("Unable to create capture readback texture")?);
        }
        let staging = self.staging.as_ref().unwrap();
        let region = D3D11_BOX {
            left: 0,
            top: 0,
            front: 0,
            right: width,
            bottom: height,
            back: 1,
        };
        let destination: ID3D11Resource = staging.cast()?;
        let source: ID3D11Resource = texture.cast()?;
        self.context
            .CopySubresourceRegion(&destination, 0, 0, 0, 0, &source, 0, Some(&region));
        let mut pixels = RgbaImage::new(width, height);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        self.context
            .Map(&destination, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        struct Unmap<'a>(&'a ID3D11DeviceContext, &'a ID3D11Resource);
        impl Drop for Unmap<'_> {
            fn drop(&mut self) {
                unsafe {
                    self.0.Unmap(self.1, 0);
                }
            }
        }
        let _unmap = Unmap(&self.context, &destination);
        anyhow::ensure!(
            !mapped.pData.is_null() && mapped.RowPitch as usize >= width as usize * 4,
            "Invalid capture readback pitch"
        );
        for y in 0..height as usize {
            let input = std::slice::from_raw_parts(
                (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                width as usize * 4,
            );
            let output = &mut pixels.as_mut()[y * width as usize * 4..(y + 1) * width as usize * 4];
            bgra_to_straight_rgba(input, output);
        }
        Ok(pixels)
    }
}

fn bgra_to_straight_rgba(input: &[u8], output: &mut [u8]) {
    for (src, dst) in input.chunks_exact(4).zip(output.chunks_exact_mut(4)) {
        let alpha = src[3] as u32;
        if alpha == 0 {
            dst.fill(0);
            continue;
        }
        if alpha == 255 {
            dst.copy_from_slice(&[src[2], src[1], src[0], 255]);
            continue;
        }
        let straight = |value: u8| ((value as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
        dst.copy_from_slice(&[straight(src[2]), straight(src[1]), straight(src[0]), src[3]]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readback_swaps_channels_and_unpremultiplies_transparent_edges() {
        let mut rgba = [0; 12];
        bgra_to_straight_rgba(
            &[10, 20, 30, 255, 16, 32, 64, 128, 255, 0, 20, 0],
            &mut rgba,
        );
        assert_eq!(rgba, [30, 20, 10, 255, 128, 64, 32, 128, 0, 0, 0, 0]);
    }
}
