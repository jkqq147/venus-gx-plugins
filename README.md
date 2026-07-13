# Venus GX Plugins

面向 Victron CCGX 的轻量插件中心。

安装 Plugin Manager 后，可以直接在 GX 屏幕的 `Settings > Plugins` 中刷新目录、安装插件、启用或关闭插件，以及确认卸载。最后一次有效目录会保存在本机，断网时仍可查看和管理已经安装的插件。

Plugin Manager 只安装管理平台，不会预装任何插件。

## 支持设备

- CCGX
- Venus OS v3.55
- ARMv7 (`armv7l`)

## 安装

通过 SSH 登录 CCGX，然后运行：

```sh
curl -fL https://venus-gx-plugins.pages.dev/releases/download/v0.1.9/venus-plugin-manager-armv7.bin -o /tmp/venus-plugin-manager
chmod 0755 /tmp/venus-plugin-manager
/tmp/venus-plugin-manager install-manager
```

安装完成后进入 `Settings > Plugins`，点击 `Refresh` 获取插件目录。以后有新版本时，也可以在此直接更新 Plugin Manager。

## 当前插件

### TPMS

通过 CCGX 的蓝牙读取 BLE 胎压传感器，在主 Dashboard 显示胎压概览，并从 Device List 直接进入轮胎绑定、传感器扫描和诊断页面。

## 文档

[项目架构](docs/ARCHITECTURE.md) · [插件包格式](docs/PLUGIN-FORMAT.md)

## 许可证

MIT
