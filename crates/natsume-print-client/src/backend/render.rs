//! The Windows PDF renderer is OS-provided; no downloadable PDF runtime is used.
use snafu::ResultExt as _;
use windows::{
    Data::Pdf::{PdfDocument, PdfPage, PdfPageRenderOptions},
    Graphics::Imaging::{
        BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat, BitmapTransform, ColorManagementMode,
        ExifOrientationMode,
    },
    Storage::Streams::{DataWriter, InMemoryRandomAccessStream},
    UI::Color,
};

use crate::{Document, InvalidSnafu, NativeSnafu, Result};

pub(super) struct Pdf {
    document: PdfDocument,
    // Keep the source stream alive for the entire PDF rendering session.
    _source: InMemoryRandomAccessStream,
    pub(super) pages: u32,
}

struct Page(PdfPage);
impl Drop for Page {
    fn drop(&mut self) {
        if self.0.Close().is_err() {
            tracing::warn!("closing the PDF page failed");
        }
    }
}

pub(super) struct Raster {
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) pixels: Vec<u8>,
}

impl Pdf {
    pub(super) fn load(document: &Document) -> Result<Self> {
        let load = || -> windows::core::Result<_> {
            let stream = InMemoryRandomAccessStream::new()?;
            let writer = DataWriter::CreateDataWriter(&stream)?;
            writer.WriteBytes(document.bytes())?;
            writer.StoreAsync()?.join()?;
            writer.FlushAsync()?.join()?;
            writer.DetachStream()?;
            stream.Seek(0)?;
            let pdf = PdfDocument::LoadFromStreamAsync(&stream)?.join()?;
            Ok((pdf, stream))
        };
        let (pdf, stream) = load().context(NativeSnafu {
            operation: "load PDF",
        })?;
        let pages = pdf.PageCount().context(NativeSnafu {
            operation: "count PDF pages",
        })?;
        if pages == 0
            || pdf.IsPasswordProtected().context(NativeSnafu {
                operation: "inspect PDF protection",
            })?
        {
            return InvalidSnafu {
                field: "PDF pages or protection",
            }
            .fail();
        }
        for index in 0..pages {
            let page = Page(pdf.GetPage(index).context(NativeSnafu {
                operation: "open PDF page",
            })?);
            let size = page.0.Size().context(NativeSnafu {
                operation: "inspect PDF size",
            })?;
            if !a4(size.Width, size.Height) {
                return InvalidSnafu {
                    field: "A4 page size",
                }
                .fail();
            }
        }
        Ok(Self {
            document: pdf,
            _source: stream,
            pages,
        })
    }

    pub(super) fn render(&self, index: u32) -> Result<Raster> {
        let render = || -> windows::core::Result<_> {
            let page = Page(self.document.GetPage(index)?);
            let size = page.0.Size()?;
            let (width, height) = if size.Width <= size.Height {
                (2480, 3508)
            } else {
                (3508, 2480)
            };
            let options = PdfPageRenderOptions::new()?;
            options.SetDestinationWidth(width)?;
            options.SetDestinationHeight(height)?;
            options.SetBackgroundColor(Color {
                A: 255,
                R: 255,
                G: 255,
                B: 255,
            })?;
            let output = InMemoryRandomAccessStream::new()?;
            page.0
                .RenderWithOptionsToStreamAsync(&output, &options)?
                .join()?;
            output.Seek(0)?;
            let decoder = BitmapDecoder::CreateAsync(&output)?.join()?;
            Ok((width, height, decode_pixels(&decoder, width, height)?))
        };
        let (width, height, mut pixels) = render().context(NativeSnafu {
            operation: "render PDF page",
        })?;
        if pixels.len() != width as usize * height as usize * 4 {
            return InvalidSnafu {
                field: "PDF raster",
            }
            .fail();
        }
        // Grayscale pixels also protect monochrome output when a driver ignores dmColor.
        for pixel in pixels.chunks_exact_mut(4) {
            let weighted =
                u32::from(pixel[0]) * 29 + u32::from(pixel[1]) * 150 + u32::from(pixel[2]) * 77;
            // The weights sum to 256, so the shifted value always fits in one byte.
            let gray = (weighted >> 8).to_le_bytes()[0];
            pixel[..3].fill(gray);
            pixel[3] = 255;
        }
        Ok(Raster {
            width: i32::try_from(width).map_err(|_| {
                InvalidSnafu {
                    field: "raster width",
                }
                .build()
            })?,
            height: i32::try_from(height).map_err(|_| {
                InvalidSnafu {
                    field: "raster height",
                }
                .build()
            })?,
            pixels,
        })
    }
}

fn decode_pixels(
    decoder: &BitmapDecoder,
    width: u32,
    height: u32,
) -> windows::core::Result<Vec<u8>> {
    // The PDF renderer uses logical units; request the final physical pixel size explicitly.
    let transform = BitmapTransform::new()?;
    transform.SetScaledWidth(width)?;
    transform.SetScaledHeight(height)?;
    let provider = decoder
        .GetPixelDataTransformedAsync(
            BitmapPixelFormat::Bgra8,
            BitmapAlphaMode::Ignore,
            &transform,
            ExifOrientationMode::IgnoreExifOrientation,
            ColorManagementMode::DoNotColorManage,
        )?
        .join()?;
    Ok(provider.DetachPixelData()?.to_vec())
}

fn a4(width: f32, height: f32) -> bool {
    // WinRT reports 96-DPI DIPs; allow the same 0.1-PDF-point rounding as Server.
    let portrait = |w: f32, h: f32| (w - 793.7008).abs() <= 0.134 && (h - 1122.5197).abs() <= 0.134;
    portrait(width, height) || portrait(height, width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_renderer_reads_both_orientations_and_produces_grayscale_pixels() -> Result<()> {
        let _apartment = super::super::Apartment::initialize()?;
        let directory = tempfile::tempdir().context(crate::IoSnafu {
            operation: "test directory",
        })?;
        let path = directory.path().join("a4.pdf");
        std::fs::write(&path, include_bytes!("../../tests/fixtures/a4.pdf")).context(
            crate::IoSnafu {
                operation: "test PDF",
            },
        )?;
        let document = Document::load(&path, &uuid::Uuid::now_v7().to_string())?;
        let pdf = Pdf::load(&document)?;
        assert_eq!(pdf.pages, 2);
        for (index, expected) in [(0, (2480, 3508)), (1, (3508, 2480))] {
            let raster = pdf.render(index)?;
            assert_eq!((raster.width, raster.height), expected);
            assert!(
                raster
                    .pixels
                    .chunks_exact(4)
                    .all(|pixel| pixel[0] == pixel[1] && pixel[1] == pixel[2] && pixel[3] == 255)
            );
            assert!(raster.pixels.chunks_exact(4).any(|pixel| pixel[0] < 200));
        }
        Ok(())
    }

    #[test]
    fn dpi_scaled_bitmaps_are_normalized_to_target_pixel_dimensions() -> Result<()> {
        use windows::Graphics::Imaging::BitmapEncoder;

        let _apartment = super::super::Apartment::initialize()?;
        for (width, height) in [(16_u32, 24_u32), (24, 16)] {
            for scale in [100_u32, 150, 200] {
                let source_width = width * scale / 100;
                let source_height = height * scale / 100;
                let mut source = vec![255; source_width as usize * source_height as usize * 4];
                for y in 0..source_height {
                    for x in 0..source_width / 2 {
                        let offset = (y as usize * source_width as usize + x as usize) * 4;
                        source[offset..offset + 3].fill(0);
                    }
                }
                let normalize = || -> windows::core::Result<_> {
                    let stream = InMemoryRandomAccessStream::new()?;
                    let encoder =
                        BitmapEncoder::CreateAsync(BitmapEncoder::PngEncoderId()?, &stream)?
                            .join()?;
                    let dpi = 96.0 * f64::from(scale) / 100.0;
                    encoder.SetPixelData(
                        BitmapPixelFormat::Bgra8,
                        BitmapAlphaMode::Ignore,
                        source_width,
                        source_height,
                        dpi,
                        dpi,
                        &source,
                    )?;
                    encoder.FlushAsync()?.join()?;
                    stream.Seek(0)?;
                    let decoder = BitmapDecoder::CreateAsync(&stream)?.join()?;
                    assert_eq!(decoder.PixelWidth()?, source_width);
                    assert_eq!(decoder.PixelHeight()?, source_height);
                    decode_pixels(&decoder, width, height)
                };
                let pixels = normalize().context(NativeSnafu {
                    operation: "normalize DPI-scaled bitmap fixture",
                })?;
                assert_eq!(
                    pixels.len(),
                    width as usize * height as usize * 4,
                    "wrong pixel count at {scale}% for {width}x{height}"
                );
                assert_eq!(&pixels[..3], &[0, 0, 0]);
                assert_eq!(
                    &pixels[pixels.len() - 4..pixels.len() - 1],
                    &[255, 255, 255]
                );
            }
        }
        Ok(())
    }

    #[test]
    fn accepts_a4_orientations_and_rejects_invalid_page_sizes() {
        assert!(a4(793.7008, 1122.5197));
        assert!(a4(1122.5197, 793.7008));
        assert!(!a4(816.0, 1056.0));
        assert!(!a4(f32::NAN, 1122.5197));
    }
}
