//! Native presentation adapter. Business transitions remain in the backend library.
mod application;
mod controller;
mod images;

use snafu::Snafu;

slint::slint! {
    import { PhotoWindow, ContestantWindow } from "window.slint";
    export { PhotoWindow, ContestantWindow }
}

#[derive(Debug, Snafu)]
pub(crate) enum Error {
    #[snafu(display("Photo application stopped after a backend failure; see the diagnostic log"))]
    ApplicationStopped,
    #[snafu(display("选手窗口正在关闭，请稍后重新打开"))]
    DisplayClosing,
    #[snafu(display("cannot create or run the Photo windows"))]
    Platform { source: slint::PlatformError },
    #[snafu(display("the selected renderer cannot acknowledge completed frames"))]
    Rendering {
        source: slint::SetRenderingNotifierError,
    },
    #[snafu(display("cannot start the Photo application thread"))]
    Thread { source: std::io::Error },
    #[snafu(display("the Photo application thread failed"))]
    ThreadPanicked,
}

pub(crate) fn run() -> Result<(), Error> {
    controller::run()
}
