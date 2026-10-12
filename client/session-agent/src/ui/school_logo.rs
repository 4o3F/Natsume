use std::{cell::RefCell, rc::Rc};

use image::{Rgba, RgbaImage, imageops::FilterType};
use slint::ComponentHandle as _;

use super::SessionWindow;

/// Retains the original raster logo and resamples it to its physical contain box.
///
/// # Errors
///
/// Returns a fixed error if the source cannot be decoded into RGBA pixels.
pub fn install(window: &SessionWindow, source: &slint::Image) -> Result<(), &'static str> {
    let pixels = source.to_rgba8().ok_or("school logo pixels unavailable")?;
    let source = RgbaImage::from_raw(pixels.width(), pixels.height(), pixels.as_bytes().to_vec())
        .ok_or("school logo pixels invalid")?;
    if source.width() == 0 || source.height() == 0 {
        return Err("school logo pixels empty");
    }
    let source = Rc::new(source);
    let last_size = Rc::new(RefCell::new(None));
    let weak = window.as_weak();
    let update = move |requested: i32| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let size = target_size(&source, requested);
        if *last_size.borrow() == Some(size) {
            return;
        }
        let pixels = resample(&source, size);
        let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
            pixels.as_raw(),
            size.0,
            size.1,
        );
        window.set_school_logo(slint::Image::from_rgba8(buffer));
        *last_size.borrow_mut() = Some(size);
    };
    update(window.get_school_logo_pixels());
    // Replacing the callback also releases the previous school's retained source.
    window.on_school_logo_size_changed(update);
    Ok(())
}

pub(super) fn reset(window: &SessionWindow, source: slint::Image) {
    window.on_school_logo_size_changed(|_| {});
    window.set_school_logo(source);
}

fn target_size(source: &RgbaImage, requested: i32) -> (u32, u32) {
    let longest = source.width().max(source.height());
    let edge = u32::try_from(requested).unwrap_or(1).clamp(1, longest);
    let scaled = |side: u32| {
        u32::try_from(
            (u64::from(side) * u64::from(edge) + u64::from(longest / 2)) / u64::from(longest),
        )
        .unwrap_or(edge)
        .max(1)
    };
    (scaled(source.width()), scaled(source.height()))
}

fn resample(source: &RgbaImage, size: (u32, u32)) -> RgbaImage {
    if size == source.dimensions() {
        return source.clone();
    }
    // Filter premultiplied alpha, so invisible RGB does not bleed into crest edges.
    // image's float resize clamps its result to [0, 1]. Reserve headroom for
    // Lanczos's negative lobes; clipping alpha before division would change color.
    let premultiplied = image::Rgba32FImage::from_fn(source.width(), source.height(), |x, y| {
        let pixel = source.get_pixel(x, y);
        let alpha = f32::from(pixel[3]) / 255.0;
        let encode = |value: f32| 0.25 + 0.25 * value;
        Rgba([
            encode(f32::from(pixel[0]) / 255.0 * alpha),
            encode(f32::from(pixel[1]) / 255.0 * alpha),
            encode(f32::from(pixel[2]) / 255.0 * alpha),
            encode(alpha),
        ])
    });
    let filtered = image::imageops::resize(&premultiplied, size.0, size.1, FilterType::Lanczos3);
    drop(premultiplied);
    let straight = image::Rgba32FImage::from_fn(size.0, size.1, |x, y| {
        let pixel = filtered.get_pixel(x, y);
        let decode = |value: f32| (value - 0.25) * 4.0;
        let alpha = decode(pixel[3]);
        let channel = |value: f32| {
            if alpha > 0.0 {
                (decode(value) / alpha).clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        Rgba([
            channel(pixel[0]),
            channel(pixel[1]),
            channel(pixel[2]),
            alpha.clamp(0.0, 1.0),
        ])
    });
    image::DynamicImage::ImageRgba32F(straight).into_rgba8()
}

#[cfg(test)]
mod tests {
    use super::{resample, target_size};
    use image::{Rgba, RgbaImage};

    #[test]
    fn contain_preserves_both_edges_of_a_non_square_logo() {
        let source = RgbaImage::from_fn(80, 40, |x, _| {
            if x < 40 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 255])
            }
        });
        assert_eq!(target_size(&source, 20), (20, 10));
        let resized = resample(&source, (20, 10));
        assert_eq!(resized.get_pixel(0, 5), &Rgba([255, 0, 0, 255]));
        assert_eq!(resized.get_pixel(19, 5), &Rgba([0, 0, 255, 255]));
        assert_eq!(target_size(&RgbaImage::new(40, 80), 20), (10, 20));
    }

    #[test]
    fn target_is_bounded_by_original_dimensions_not_a_previous_thumbnail() {
        let source = RgbaImage::new(512, 256);
        assert_eq!(target_size(&source, 192), (192, 96));
        assert_eq!(target_size(&source, 384), (384, 192));
        assert_eq!(target_size(&source, 1024), (512, 256));
        assert_eq!(target_size(&source, 0), (1, 1));
        assert_eq!(target_size(&source, -1), (1, 1));
        assert_eq!(resample(&source, (512, 256)), source);
    }

    #[test]
    fn downsampling_filters_high_frequency_detail() {
        let source = RgbaImage::from_fn(64, 64, |x, y| {
            let value = if (x + y) % 2 == 0 { 0 } else { 255 };
            Rgba([value, value, value, 255])
        });
        for pixel in resample(&source, (4, 4)).pixels() {
            assert!((120..=135).contains(&pixel[0]));
            assert_eq!(pixel[0], pixel[1]);
            assert_eq!(pixel[1], pixel[2]);
            assert_eq!(pixel[3], 255);
        }
    }

    #[test]
    fn alpha_step_preserves_non_saturated_color() {
        let source = RgbaImage::from_fn(64, 64, |x, _| {
            Rgba([128, 64, 32, if x < 32 { 255 } else { 0 }])
        });
        let filtered = resample(&source, (32, 32));
        let mut minimum = [255; 3];
        let mut maximum = [0; 3];
        for pixel in filtered.pixels().filter(|pixel| pixel[3] > 0) {
            for channel in 0..3 {
                minimum[channel] = minimum[channel].min(pixel[channel]);
                maximum[channel] = maximum[channel].max(pixel[channel]);
            }
        }
        assert_eq!(minimum, [128, 64, 32]);
        assert_eq!(maximum, [128, 64, 32]);
        for pixel in filtered.pixels().filter(|pixel| pixel[3] > 0) {
            assert!(pixel[0].abs_diff(128) <= 1);
            assert!(pixel[1].abs_diff(64) <= 1);
            assert!(pixel[2].abs_diff(32) <= 1);
        }
    }

    #[test]
    fn invisible_rgb_does_not_bleed_into_translucent_edges() {
        let source = RgbaImage::from_fn(64, 64, |x, y| {
            if (x + y) % 2 == 0 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 0])
            }
        });
        for pixel in resample(&source, (4, 4)).pixels() {
            assert_eq!(pixel[0], 255);
            assert_eq!(pixel[1], 0);
            assert_eq!(pixel[2], 0);
            assert!((120..=135).contains(&pixel[3]));
        }
    }
}
