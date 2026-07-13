# `.vplugin` 插件包格式

`.vplugin` 是使用 gzip 压缩的 tar 归档。包内只能出现 `manifest.json`、`bin/` 和 `qml/`，不能携带独立安装脚本、远程 shell hook、Python 运行环境或其他顶层目录。

```text
plugin.vplugin
├── manifest.json
├── bin/
│   └── <static-armv7-binary>
└── qml/
    └── <plugin-pages.qml>
```

当前实现不执行包内任何生命周期 hook。对于 `native-service`，只有 manifest 声明的可执行文件会被设置为可执行权限；其他普通文件保持不可执行。

## Manifest

每个 manifest 使用 `schema: 1`，插件 ID 只能包含小写 ASCII 字母、数字和连字符。

```json
{
  "schema": 1,
  "id": "tpms",
  "name": "TPMS",
  "version": "0.1.0",
  "runtime": {
    "kind": "native-service",
    "executable": "bin/venus-tpms-ble"
  },
  "settings": {
    "enabled_path": "/Settings/Plugins/tpms/Enabled"
  },
  "ui": {
    "settings_page": "qml/PageTpmsSettings.qml",
    "dashboard_component": "qml/OverviewTpms.qml"
  }
}
```

支持两种 runtime：

- `native-service`：必须提供 manifest 中 `bin/` 下声明的自包含可执行文件。
- `qml-only`：不包含持久后台服务。

Manifest 引用的 Settings 页面和 Dashboard 组件必须位于 `qml/` 下并真实存在。Enabled 路径必须严格等于 `/Settings/Plugins/<plugin-id>/Enabled`。

## 安全校验

安装事务在修改当前 Registry 前完成以下校验：

- 包 SHA-256 必须等于 Catalog 提供的预期值。
- 包中 manifest 的 ID 和版本必须等于 Catalog 条目。
- 归档路径必须是 UTF-8 安全相对路径，拒绝绝对路径、`..`、反斜杠和重复路径。
- 拒绝符号链接、硬链接、设备、FIFO 和其他非普通文件类型。
- 拒绝 `manifest.json`、`bin/`、`qml/` 之外的内容。
- 最多允许 512 个归档条目、128 MiB 压缩包和 256 MiB 解压后普通文件。
- manifest 合同、运行文件和所有被引用 QML 文件必须完整有效。

HTTPS 下载与本地 Catalog 缓存尚未实现；当前本地安装入口仍强制要求预期 ID、版本和 SHA-256。包签名校验也尚未实现，正式公开发布前必须增加。

## Registry 与安装事务

Registry 使用 `schema: 1`，以插件 ID 为键保存完整 manifest、enabled 期望状态、包 SHA-256 和实际 payload 相对路径。

安装和升级采用以下提交顺序：

1. 在状态根目录的 `staging/` 中复制包并同步到磁盘，同时计算 SHA-256。
2. 在 staging 中安全解包并完成全部合同和文件校验。
3. 将 payload 移动到 `plugins/<id>/<sha256>/` 不可变目录。
4. 写入并同步临时 Registry 文件，再原子替换 `registry.json`。
5. 提交成功后清理旧 payload；升级会保留原有 enabled 期望状态。

第 4 步是事务提交点。提交前失败会保留旧 Registry 和旧 payload；卸载则先原子移除 Registry 条目，再清理已失去引用的 payload。

目前 enabled 只是本地期望状态。后续 D-Bus 服务负责把它与 Venus Settings、runit 服务状态和 QML 可见性统一协调。
