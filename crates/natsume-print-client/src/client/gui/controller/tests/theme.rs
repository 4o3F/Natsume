use super::*;
use slint::private_unstable_api::re_exports::ColorScheme;

fn dark_pixels(
    pixels: &[slint::Rgb8Pixel],
    width: usize,
    x: std::ops::Range<usize>,
    y: std::ops::Range<usize>,
) -> usize {
    y.flat_map(|row| x.clone().map(move |column| pixels[row * width + column]))
        .filter(|pixel| pixel.r < 100 && pixel.g < 100 && pixel.b < 100)
        .count()
}

#[test]
fn white_interface_keeps_unfocused_text_and_exit_dialog_readable_on_a_dark_system()
-> std::result::Result<(), TestError> {
    let window = Rc::new(RefCell::new(None));
    // Only the fixture changes backend theme state; it does not change the actual OS theme.
    let context =
        slint::private_unstable_api::re_exports::SlintContext::new(Box::new(HeadlessPlatform {
            window: window.clone(),
            ..HeadlessPlatform::default()
        }));
    context.set_color_scheme(ColorScheme::Dark);
    let ui = PrintWindow::new().context(GuiSnafu).context(ClientSnafu)?;
    ui.set_config_path("C:\\Print\\client.toml".into());
    ui.show().context(GuiSnafu).context(ClientSnafu)?;
    let window = window
        .borrow()
        .clone()
        .ok_or_else(|| {
            AttentionSnafu {
                reason: "no theme test window",
            }
            .build()
        })
        .context(ClientSnafu)?;
    let width = usize::try_from(window.size().width).context(DimensionsSnafu)?;
    let height = usize::try_from(window.size().height).context(DimensionsSnafu)?;
    let mut pixels = vec![slint::Rgb8Pixel::default(); width * height];
    for scheme in [ColorScheme::Dark, ColorScheme::Light, ColorScheme::Dark] {
        context.set_color_scheme(scheme);
        window.renderer.render(&mut pixels, width);
        assert!(
            dark_pixels(&pixels, width, 24..320, 50..82) > 50,
            "the page heading must remain dark on white"
        );
        assert!(
            dark_pixels(&pixels, width, 36..380, 205..229) > 20,
            "unfocused input text must remain visible without clicking"
        );
    }
    ui.set_exit_confirm(true);
    window.renderer.render(&mut pixels, width);
    assert!(
        dark_pixels(&pixels, width, 210..640, 92..145) > 50,
        "exit confirmation instructions must remain readable on white"
    );
    ui.hide().context(GuiSnafu).context(ClientSnafu)?;
    Ok(())
}
