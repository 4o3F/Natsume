# Natsume Windows 打印客户端

本便携包面向 Windows 10／11 x64 的已登录用户会话，包含 `natsume-print-client.exe`、项目许可证、包版本信息和文件校验和。它不是安装器，不安装打印机驱动、证书或运行库，不配置登录启动，也不包含服务器配置、配对凭据、PDF 或本地任务记录。

## 校验与启动

从可信的 Natsume GitHub Release 下载 ZIP 和 `SHA256SUMS`，用 `Get-FileHash -Algorithm SHA256` 核对 ZIP 摘要后解压。包内 `SHA256SUMS` 用于核对解压后的文件；同渠道校验和只能帮助核对下载字节，不能代替对发布来源的信任。`package.json` 记录包版本、平台、架构和程序清单，包版本可能与程序内的 Cargo workspace 版本不同。

直接打开 `natsume-print-client.exe` 进入配置界面；已有部署应明确加载原配置和原状态目录，也可以在 PowerShell 中运行：

```powershell
.\natsume-print-client.exe --config 'C:\Natsume\print-client.toml'
```

服务器必须是可信 HTTPS origin，Windows 队列必须已在本机安装。配置由部署方提供，不要把凭据放入命令行或配置示例；管理员在 Web 中签发配对码，客户端将凭据保存到当前用户的 Windows PasswordVault。首次配对后默认暂停，需要明确恢复领取。

完整的配置、安全和操作说明见 [Windows 打印客户端说明](https://github.com/4o3F/Natsume/blob/v2/crates/natsume-print-client/README.md)。运行时使用系统 PDF 渲染和打印接口，不下载 PDF 渲染器。包未捆绑 Visual C++ 运行库；干净 Windows 10／11 环境的运行库需求及实际驱动兼容性仍须单独验收，CI 构建成功不证明所有目标机器都能直接运行。

## 更新与数据保留

更新前，从窗口或托盘确认正常退出；关闭主窗口可能只是隐藏到托盘。等待当前程序退出后，将新包解压到独立程序目录，并继续使用原配置、原状态目录和原 Windows 用户。不要让新旧程序同时占用同一个状态目录，不要通过重新配对、新建空状态目录或删除记录来恢复任务。

配置、凭据和任务记录不属于本包。移除旧程序目录前确认其中没有自行放入的配置或业务数据；本包没有卸载或数据清理脚本。升级协议兼容性以对应 Release 说明为准，不能仅凭“便携包”推断任意版本可混用。

## 运行边界

暂停只阻止新领取，不中止当前工作；正常退出会等待已启动的原生调用。当前保留控制台输出，不要通过关闭控制台强制结束进程。打印系统报告完成不证明纸张已交付；队列中的作业消失也不证明成功。包不提供取消任务、重置次数或手动重印入口。

本包附项目 AGPL 许可证；完整源码及 Cargo 依赖和许可声明见 [Natsume 仓库](https://github.com/4o3F/Natsume)。随包提供项目许可证不构成全部传递依赖的完整许可审计。
