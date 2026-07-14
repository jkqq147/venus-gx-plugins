# Venus GX Plugins

面向 Victron CCGX 的轻量插件中心。

安装 Plugin Manager 后，可以直接在 GX 屏幕的 `Settings > Plugins` 中获取、安装、启用、关闭或卸载插件。断网时仍可查看和管理已经安装的插件。

Plugin Manager 只安装管理平台，不会预装任何插件。

## 支持设备

- CCGX
- Venus OS v3.55
- ARMv7 (`armv7l`)

## 安装

### 1. 开启 CCGX 的 SSH

在 CCGX 上进入 `Settings > General`：

1. 将 `Access level` 设为 `User and installer`，密码为 `ZZZ`。
2. 返回 `General` 页面并选中 `Access level`，不要进入该选项；长按面板右键，直到它变为 `Superuser`。
3. 打开 `Set root password`，设置一个至少 6 位的强 root 密码。
4. 启用 `SSH on LAN`。

完整步骤见 [Victron 官方 Root Access 文档](https://www.victronenergy.com/live/ccgx:root_access)。

### 2. 登录 CCGX

确保电脑与 CCGX 位于同一局域网。在 CCGX 的网络设置中查看 IP 地址，然后从电脑终端登录，例如：

```sh
ssh root@192.168.1.23
```

将示例 IP 替换为你的 CCGX 地址，首次连接时确认设备指纹并输入刚设置的 root 密码。

### 3. 安装 Plugin Manager

登录成功后运行：

```sh
curl -fL https://venus-gx-plugins.pages.dev/releases/download/v0.1.13/venus-plugin-manager-armv7.bin -o /tmp/venus-plugin-manager
chmod 0755 /tmp/venus-plugin-manager
/tmp/venus-plugin-manager install-manager
```

安装完成后进入 `Settings > Plugins`，点击 `Check for updates` 获取插件目录。以后有新版本时，也可以在此直接更新 Plugin Manager。

## 当前插件

| 插件 | 用途 |
| --- | --- |
| [TPMS](plugins/tpms/README.md) | 蓝牙胎压监测 |
| [Rathole](plugins/rathole/README.md) | 将 GX 内网服务映射到公网 |

## 许可证

MIT
