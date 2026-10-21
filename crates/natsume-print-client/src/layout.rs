use crate::{InvalidSnafu, Result};

#[derive(Debug, PartialEq, Eq)]
pub struct PageLayout {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Fits the whole rendered page inside the printable area, without cropping.
///
/// # Errors
/// Rejects zero or negative dimensions.
pub fn fit_page(
    page_width: i32,
    page_height: i32,
    printable_width: i32,
    printable_height: i32,
) -> Result<PageLayout> {
    if [page_width, page_height, printable_width, printable_height]
        .iter()
        .any(|value| *value <= 0)
    {
        return InvalidSnafu {
            field: "page geometry",
        }
        .fail();
    }
    let (pw, ph, aw, ah) = (
        i64::from(page_width),
        i64::from(page_height),
        i64::from(printable_width),
        i64::from(printable_height),
    );
    let (width, height) = if pw * ah > ph * aw {
        (aw, (ph * aw / pw).max(1))
    } else {
        ((pw * ah / ph).max(1), ah)
    };
    // Both fitted dimensions are bounded by the corresponding positive i32 area.
    let width = i32::try_from(width).map_err(|_| {
        InvalidSnafu {
            field: "page geometry",
        }
        .build()
    })?;
    let height = i32::try_from(height).map_err(|_| {
        InvalidSnafu {
            field: "page geometry",
        }
        .build()
    })?;
    Ok(PageLayout {
        x: (printable_width - width) / 2,
        y: (printable_height - height) / 2,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portrait_and_landscape_are_centered_without_clipping() -> Result<()> {
        assert_eq!(
            fit_page(210, 297, 190, 277)?,
            PageLayout {
                x: 0,
                y: 4,
                width: 190,
                height: 268
            }
        );
        assert_eq!(
            fit_page(297, 210, 190, 277)?,
            PageLayout {
                x: 0,
                y: 71,
                width: 190,
                height: 134
            }
        );
        assert_eq!(fit_page(i32::MAX, 1, 1, i32::MAX)?.width, 1);
        assert!(fit_page(0, 10, 10, 10).is_err());
        assert!(fit_page(10, 10, -1, 10).is_err());
        Ok(())
    }
}
