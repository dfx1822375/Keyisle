use image::{
    DynamicImage, ImageDecoder, ImageEncoder, ImageFormat, ImageReader, Limits,
    codecs::png::{CompressionType, FilterType, PngEncoder},
    imageops::FilterType as ResizeFilter,
};
use slint::{Image, SharedPixelBuffer};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Cursor, Read, Write},
    path::Path,
};
use windows_password_manager::settings::{WALLPAPER_FILE, WALLPAPER_LIMIT, WALLPAPER_TEMP};

pub const DEFAULT_TRANSPARENCY: i32 = 48;
pub const MAX_TRANSPARENCY: i32 = 90;
const TRANSPARENCY_FORMAT: u8 = 0x80;
const SOURCE_LIMIT: u64 = 16 * 1024 * 1024;
const MAX_SOURCE_SIDE: u32 = 8192;
const MAX_SOURCE_PIXELS: u64 = 16 * 1024 * 1024;
const DISPLAY_WIDTH: u32 = 1920;
const DISPLAY_HEIGHT: u32 = 1080;

pub struct ImportedWallpaper {
    pub pixels: DynamicImage,
    pub png: Vec<u8>,
}

fn read_bounded(file: File, limit: u64) -> io::Result<Vec<u8>> {
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid image size",
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid image size",
        ));
    }
    Ok(bytes)
}

fn decode(
    bytes: &[u8],
    max_width: u32,
    max_height: u32,
    max_pixels: u64,
) -> Result<DynamicImage, String> {
    let failed = || "无法读取图片，请选择有效的 PNG 或 JPEG。".to_string();
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| failed())?;
    if !matches!(reader.format(), Some(ImageFormat::Png | ImageFormat::Jpeg)) {
        return Err(failed());
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(max_width);
    limits.max_image_height = Some(max_height);
    limits.max_alloc = Some(96 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|_| failed())?;
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) > max_pixels {
        return Err("图片像素过多，请缩小后再选择。".into());
    }
    let orientation = decoder.orientation().map_err(|_| failed())?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(|_| failed())?;
    image.apply_orientation(orientation);
    Ok(image)
}

pub fn slint_image(pixels: DynamicImage) -> Image {
    match pixels {
        DynamicImage::ImageRgb8(rgb) => Image::from_rgb8(SharedPixelBuffer::clone_from_slice(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
        )),
        other => {
            let rgba = other.into_rgba8();
            Image::from_rgba8(SharedPixelBuffer::clone_from_slice(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
            ))
        }
    }
}

pub fn import(path: &Path) -> Result<ImportedWallpaper, String> {
    let bytes = File::open(path)
        .and_then(|file| read_bounded(file, SOURCE_LIMIT))
        .map_err(|_| "无法读取图片，请选择不超过 16 MB 的 PNG 或 JPEG。")?;
    let mut image = decode(&bytes, MAX_SOURCE_SIDE, MAX_SOURCE_SIDE, MAX_SOURCE_PIXELS)?;
    if image.width() > DISPLAY_WIDTH || image.height() > DISPLAY_HEIGHT {
        image = image.resize(DISPLAY_WIDTH, DISPLAY_HEIGHT, ResizeFilter::Triangle);
    }
    let pixels = if image.color().has_alpha() {
        DynamicImage::ImageRgba8(image.into_rgba8())
    } else {
        DynamicImage::ImageRgb8(image.into_rgb8())
    };
    let mut png = Vec::new();
    PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::Adaptive)
        .write_image(
            pixels.as_bytes(),
            pixels.width(),
            pixels.height(),
            pixels.color().into(),
        )
        .map_err(|_| "无法处理背景图片。")?;
    if png.len() as u64 + 1 > WALLPAPER_LIMIT {
        return Err("处理后的背景图片过大。".into());
    }
    Ok(ImportedWallpaper { pixels, png })
}

fn read_transparency(value: u8) -> Result<i32, String> {
    // Earlier files used the first byte for image opacity and limited it to 35%.
    let transparency = i32::from(value & !TRANSPARENCY_FORMAT);
    if value & TRANSPARENCY_FORMAT != 0 && transparency <= MAX_TRANSPARENCY {
        return Ok(transparency);
    } else if value & TRANSPARENCY_FORMAT == 0 && transparency <= 35 {
        return Ok((transparency * MAX_TRANSPARENCY + 17) / 35);
    }
    Err("本地背景透明度无效。".into())
}

fn open_saved(path: &Path) -> io::Result<File> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.file_type().is_file() || meta.len() > WALLPAPER_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid wallpaper file",
        ));
    }
    File::open(path)
}

pub fn load(root: &Path) -> Result<Option<(DynamicImage, i32)>, String> {
    let file = match open_saved(&root.join(WALLPAPER_FILE)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("无法读取本地背景图片。".into()),
    };
    let bytes = read_bounded(file, WALLPAPER_LIMIT).map_err(|_| "本地背景图片无效。")?;
    let transparency = read_transparency(*bytes.first().ok_or("本地背景图片无效。")?)?;
    let pixels = decode(
        &bytes[1..],
        DISPLAY_WIDTH,
        DISPLAY_HEIGHT,
        u64::from(DISPLAY_WIDTH * DISPLAY_HEIGHT),
    )?;
    Ok(Some((pixels, transparency)))
}

pub fn save(
    root: &Path,
    png: Option<&[u8]>,
    remove: bool,
    transparency: i32,
) -> Result<(), String> {
    let path = root.join(WALLPAPER_FILE);
    let result = if !remove {
        let temp = root.join(WALLPAPER_TEMP);
        (|| {
            let mut file = match OpenOptions::new().write(true).create_new(true).open(&temp) {
                Err(error)
                    if error.kind() == io::ErrorKind::AlreadyExists
                        && fs::symlink_metadata(&temp)?.file_type().is_file() =>
                {
                    fs::remove_file(&temp)?;
                    OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&temp)?
                }
                other => other?,
            };
            let result = (|| {
                file.write_all(&[
                    TRANSPARENCY_FORMAT | transparency.clamp(0, MAX_TRANSPARENCY) as u8
                ])?;
                if let Some(bytes) = png {
                    file.write_all(bytes)?;
                } else {
                    let mut previous = open_saved(&path)?;
                    let mut header = [0];
                    previous.read_exact(&mut header)?;
                    let copied = io::copy(&mut previous.take(WALLPAPER_LIMIT), &mut file)?;
                    if copied == 0 || copied >= WALLPAPER_LIMIT {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "invalid wallpaper data",
                        ));
                    }
                }
                file.sync_all()?;
                Ok(())
            })();
            drop(file);
            result
                .and_then(|()| fs::rename(&temp, &path))
                .inspect_err(|_| {
                    let _ = fs::remove_file(&temp);
                })
        })()
    } else {
        match fs::remove_file(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    };
    result.map_err(|_| "无法保存背景图片。".into())
}
