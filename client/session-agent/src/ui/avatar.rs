use std::{cell::RefCell, rc::Rc};

use image::{RgbaImage, imageops::FilterType};
use slint::ComponentHandle as _;

use super::SessionWindow;

/// Retains the original avatar and resamples it when its physical display size changes.
///
/// # Errors
///
/// Returns a fixed error if the source cannot be decoded into RGBA pixels.
pub fn install(window: &SessionWindow, source: &slint::Image) -> Result<(), &'static str> {
    let pixels = source.to_rgba8().ok_or("avatar pixels unavailable")?;
    let source = RgbaImage::from_raw(pixels.width(), pixels.height(), pixels.as_bytes().to_vec())
        .ok_or("avatar pixels invalid")?;
    if source.width() == 0 || source.height() == 0 {
        return Err("avatar pixels empty");
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
            size,
            size,
        );
        window.set_author_avatar(slint::Image::from_rgba8(buffer));
        *last_size.borrow_mut() = Some(size);
    };
    update(window.get_author_avatar_pixels());
    window.on_author_avatar_size_changed(update);
    Ok(())
}

fn target_size(source: &RgbaImage, requested: i32) -> u32 {
    u32::try_from(requested)
        .unwrap_or(1)
        .clamp(1, source.width().min(source.height()))
}

fn resample(source: &RgbaImage, size: u32) -> RgbaImage {
    // Match image-fit: cover without distorting a non-square preview source.
    let crop_width = source.width().min(source.height());
    let square = image::imageops::crop_imm(
        source,
        (source.width() - crop_width) / 2,
        (source.height() - crop_width) / 2,
        crop_width,
        crop_width,
    );
    image::imageops::resize(&square.to_image(), size, size, FilterType::Lanczos3)
}

#[cfg(test)]
mod tests {
    use image::{Rgba, RgbaImage};

    use super::{resample, target_size};

    #[test]
    fn physical_target_is_bounded_by_the_original_not_previous_thumbnail() {
        let source = RgbaImage::new(512, 512);
        assert_eq!(target_size(&source, 56), 56);
        assert_eq!(target_size(&source, 112), 112);
        assert_eq!(target_size(&source, 1024), 512);
        assert_eq!(target_size(&source, 0), 1);
        assert_eq!(target_size(&source, -1), 1);
        assert_eq!(resample(&source, 56).dimensions(), (56, 56));
        assert_eq!(resample(&source, 112).dimensions(), (112, 112));
    }

    #[test]
    fn downsampling_filters_high_frequency_detail_instead_of_point_sampling() {
        let source = RgbaImage::from_fn(64, 64, |x, y| {
            let value = if (x + y) % 2 == 0 { 0 } else { 255 };
            Rgba([value, value, value, 255])
        });
        let reduced = resample(&source, 4);
        for pixel in reduced.pixels() {
            assert!((120..=135).contains(&pixel[0]));
            assert_eq!(pixel[0], pixel[1]);
            assert_eq!(pixel[1], pixel[2]);
            assert_eq!(pixel[3], 255);
        }
    }

    #[test]
    fn cover_crop_preserves_the_centre_of_a_non_square_image() {
        let source = RgbaImage::from_fn(8, 4, |x, _| {
            if (2..6).contains(&x) {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 255])
            }
        });
        assert!(
            resample(&source, 4)
                .pixels()
                .all(|pixel| *pixel == Rgba([255, 0, 0, 255]))
        );
    }
}
