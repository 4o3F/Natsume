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
    #[snafu(display("所选显示器已变化，请在显示设置中重新选择操作屏和选手屏"))]
    DisplaySelection,
    #[snafu(display("选手窗口创建或定位超时，请重新打开"))]
    DisplayTimeout,
    #[snafu(display("选手窗口不在指定屏幕、已退出全屏或渲染不可用，请重新打开"))]
    DisplayInvalid,
    #[snafu(display("后台队列已满，选手窗口未连接，请稍后重新打开"))]
    DisplayQueueFull,
    #[snafu(display("后台已停止，无法连接选手窗口"))]
    DisplayWorkerStopped,
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
    #[snafu(display("cannot schedule Photo window operation"))]
    WindowEventLoop { source: slint::EventLoopError },
    #[snafu(display("cannot drag the Photo window"))]
    WindowDrag {
        source: slint::winit_030::winit::error::ExternalError,
    },
}

pub(crate) fn run() -> Result<(), Error> {
    controller::run()
}
