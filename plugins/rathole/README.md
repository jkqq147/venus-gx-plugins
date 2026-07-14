# Rathole

Rathole 是一个内网穿透工具，可以将 GX 设备上的 SSH、Web 等内网服务安全地映射到公网，方便在外部网络访问。

## 开始前

你需要先准备一台可以从公网访问的 Rathole 服务端，并取得以下信息：

- 服务端地址和控制端口，例如 `tunnel.example.com:2333`
- 服务名称，例如 `gx-ssh`
- 与服务端一致的 Token
- 服务端分配的公网访问端口

Rathole 插件只负责 GX 上的客户端，不提供公共服务器。

## 配置

通过 SSH 以 `root` 用户登录 GX，然后打开配置文件：

```sh
nano /data/venus-gx-plugins/config/rathole/client.toml
```

以公开 GX 的 SSH 服务为例，输入：

```toml
[client]
remote_addr = "tunnel.example.com:2333"

[client.services.gx-ssh]
token = "替换为你的Token"
local_addr = "127.0.0.1:22"
```

其中：

| 配置 | 含义 |
| --- | --- |
| `remote_addr` | Rathole 服务端地址和控制端口 |
| `gx-ssh` | 服务名称，必须与服务端配置一致 |
| `token` | 访问凭证，必须与服务端配置一致 |
| `local_addr` | 要公开的 GX 内网服务；`127.0.0.1:22` 表示 SSH |

公网访问端口配置在 Rathole 服务端，不需要写入 GX 的 `client.toml`。

在 nano 中按 `Ctrl+O`、回车保存，再按 `Ctrl+X` 退出。

## 启用

进入 `Settings > Plugins > Rathole`，启用插件。修改配置后，需要先关闭再重新启用 Rathole 才会载入新配置。

界面显示“运行中”表示客户端进程已经启动；是否能够从公网访问，仍需通过服务端分配的公网端口实际验证。GX 界面不会显示 Token，也不会保存连接日志。

公开服务前，请先为 SSH 或 Web 管理页面设置可靠的访问保护；SSH 建议使用密钥登录。
