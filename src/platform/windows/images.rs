#![allow(unsafe_op_in_unsafe_fn)]
use crate::{
    model::{Representation, StoredItem},
    preview::{compatibility_bytes, thumbnail_size},
};
use windows::{
    Win32::{Graphics::Imaging::*, System::Com::*},
    core::Interface,
};

pub struct Image {
    pub width: u32,
    pub height: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub pixels: Vec<u8>,
}
struct ComGuard(bool);
impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                CoUninitialize();
            }
        }
    }
}
unsafe fn com() -> Result<ComGuard, String> {
    let result = CoInitializeEx(None, COINIT_MULTITHREADED);
    if result == windows::Win32::Foundation::RPC_E_CHANGED_MODE {
        return Ok(ComGuard(false));
    }
    result.ok().map_err(|e| e.to_string())?;
    Ok(ComGuard(true))
}
fn bitmap_file(rep: &Representation) -> Result<Vec<u8>, String> {
    let data = &rep.data;
    if data.len() < 40 {
        return Err("图片数据不完整".into());
    }
    let u32at = |n| u32::from_le_bytes(data[n..n + 4].try_into().unwrap());
    let header = u32at(0) as usize;
    let bpp = u16::from_le_bytes(data[14..16].try_into().unwrap());
    let compression = u32at(16);
    if header < 40 || header > data.len() || bpp == 0 || bpp > 64 {
        return Err("图片头无效".into());
    }
    let colors = u32at(32) as usize;
    let palette = if bpp <= 8 && colors == 0 {
        1usize << bpp
    } else {
        colors
    };
    let offset = 14usize
        .checked_add(header)
        .and_then(|n| palette.checked_mul(4).and_then(|p| n.checked_add(p)))
        .and_then(|n| {
            n.checked_add(if header == 40 && compression == 3 {
                12
            } else {
                0
            })
        })
        .ok_or("图片尺寸溢出")?;
    if offset > data.len() + 14 {
        return Err("图片头无效".into());
    }
    let mut bmp = Vec::with_capacity(data.len() + 14);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((data.len() + 14) as u32).to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&(offset as u32).to_le_bytes());
    bmp.extend_from_slice(data);
    Ok(bmp)
}
unsafe fn with_frame<T>(
    rep: &Representation,
    run: impl FnOnce(&IWICImagingFactory, &IWICBitmapFrameDecode) -> Result<T, String>,
) -> Result<T, String> {
    let factory: IWICImagingFactory =
        CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| e.to_string())?;
    let wrapped;
    let data = if rep.format == "PNG" {
        &rep.data
    } else {
        wrapped = bitmap_file(rep)?;
        &wrapped
    };
    let stream = factory.CreateStream().map_err(|e| e.to_string())?;
    stream
        .InitializeFromMemory(data)
        .map_err(|e| e.to_string())?;
    let decoder = factory
        .CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)
        .map_err(|e| e.to_string())?;
    let frame = decoder.GetFrame(0).map_err(|e| e.to_string())?;
    run(&factory, &frame)
}
pub fn preview(item: &StoredItem) -> Result<Image, String> {
    unsafe {
        let _com = com()?;
        let rep = item
            .representations
            .iter()
            .find(|r| r.format == "PNG")
            .or_else(|| {
                item.representations
                    .iter()
                    .find(|r| matches!(r.format.as_str(), "CF_DIB" | "CF_DIBV5"))
            })
            .ok_or("该记录没有可解码的图片")?;
        with_frame(rep, |factory, frame| {
            let (mut source_width, mut source_height) = (0, 0);
            frame
                .GetSize(&mut source_width, &mut source_height)
                .map_err(|e| e.to_string())?;
            let (width, height) = thumbnail_size(source_width, source_height)?;
            let source: IWICBitmapSource = if (width, height) != (source_width, source_height) {
                let scaler = factory.CreateBitmapScaler().map_err(|e| e.to_string())?;
                scaler
                    .Initialize(frame, width, height, WICBitmapInterpolationModeFant)
                    .map_err(|e| e.to_string())?;
                scaler.cast().map_err(|e| e.to_string())?
            } else {
                frame.cast().map_err(|e| e.to_string())?
            };
            let converter = factory.CreateFormatConverter().map_err(|e| e.to_string())?;
            converter
                .Initialize(
                    &source,
                    &GUID_WICPixelFormat32bppBGRA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .map_err(|e| e.to_string())?;
            let mut pixels = vec![0; width as usize * height as usize * 4];
            converter
                .CopyPixels(std::ptr::null(), width * 4, &mut pixels)
                .map_err(|e| e.to_string())?;
            Ok(Image {
                width,
                height,
                source_width,
                source_height,
                pixels,
            })
        })
    }
}
/// Decode directly into a caller-owned final allocation; bounded to 64 MiB.
pub fn png_dib_writer<T>(
    rep: &Representation,
    write: impl FnOnce(usize, &dyn Fn(&mut [u8]) -> Result<(), String>) -> Result<T, String>,
) -> Result<T, String> {
    unsafe {
        let _com = com()?;
        with_frame(rep, |factory, frame| {
            let (mut width, mut height) = (0, 0);
            frame
                .GetSize(&mut width, &mut height)
                .map_err(|e| e.to_string())?;
            let bytes = compatibility_bytes(width, height)?;
            let converter = factory.CreateFormatConverter().map_err(|e| e.to_string())?;
            converter
                .Initialize(
                    frame,
                    &GUID_WICPixelFormat32bppBGRA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .map_err(|e| e.to_string())?;
            write(40 + bytes, &|dib| {
                if dib.len() != 40 + bytes {
                    return Err("图片目标缓冲区长度无效".into());
                }
                dib[..40].fill(0);
                dib[0..4].copy_from_slice(&40u32.to_le_bytes());
                dib[4..8].copy_from_slice(&(width as i32).to_le_bytes());
                dib[8..12].copy_from_slice(&(-(height as i32)).to_le_bytes());
                dib[12..14].copy_from_slice(&1u16.to_le_bytes());
                dib[14..16].copy_from_slice(&32u16.to_le_bytes());
                dib[20..24].copy_from_slice(&(bytes as u32).to_le_bytes());
                converter
                    .CopyPixels(std::ptr::null(), width * 4, &mut dib[40..])
                    .map_err(|e| e.to_string())
            })
        })
    }
}
pub fn png_is_readable(rep: &Representation) -> bool {
    unsafe {
        let Ok(_com) = com() else {
            return false;
        };
        with_frame(rep, |_, frame| {
            let (mut width, mut height) = (0, 0);
            frame
                .GetSize(&mut width, &mut height)
                .map_err(|e| e.to_string())?;
            thumbnail_size(width, height).map(|_| ())
        })
        .is_ok()
    }
}
