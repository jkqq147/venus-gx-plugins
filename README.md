# Venus GX Plugins

一个运行在 Victron CCGX 上的轻量插件中心。

安装后，可以直接从 GX 屏幕的 `Settings > Plugins` 查看和管理插件，不需要每个插件单独修改系统菜单。

## 可以做什么

- 手动刷新插件列表
- 查看和安装插件
- 启用或关闭插件
- 卸载不再需要的插件
- 断网时继续查看已经安装的插件

Plugin Manager 只负责管理插件，不会预装 TPMS、Rathole 或其他插件。

## 支持设备

第一版只支持：

- CCGX
- Venus OS v3.55

其他 GX 设备和系统版本暂未测试。

## 安装

项目仍在开发中，目前还不能安装到 CCGX。

首个可用版本完成实机测试后，会提供一个 ARMv7 安装程序。届时只需将它复制到 CCGX 并运行，然后就可以从 `Settings > Plugins` 使用 Plugin Manager。

## 插件

### TPMS

显示 BLE 胎压传感器的数据，支持轮胎绑定、状态诊断、设备页面和 Dashboard。

当前状态：核心程序已经在真实 CCGX 上运行，正在制作正式插件包。

### Rathole

用于管理 Rathole 客户端的启动、停止、运行状态和卸载。配置文件继续通过 SSH 编辑。

当前状态：正在开发原生插件包装程序。

目前还没有可以从 Plugin Manager 下载的插件。

## 开发文档

[项目架构](docs/ARCHITECTURE.md) · [插件包格式](docs/PLUGIN-FORMAT.md)

## 许可证

MIT
