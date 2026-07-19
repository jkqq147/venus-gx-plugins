# `.vplugin` 插件包格式

`.vplugin` 是使用 gzip 压缩的 tar 归档。包内只能出现 `manifest.json`、`bin/`、`qml/` 和 `licenses/`，不能携带独立安装脚本、远程 shell hook、Python 运行环境或其他顶层目录。

```text
plugin.vplugin
├── manifest.json
├── bin/
│   └── <static-armv7-binary>
├── qml/
│   └── <plugin-pages.qml>
└── licenses/
    └── <third-party-license.txt>
```

当前实现不执行包内任何生命周期 hook。对于 `native-service`，只有 manifest 声明的可执行文件会被设置为可执行权限；其他普通文件保持不可执行。

## Manifest

当前 Manager 兼容 manifest schema 1 至 5：schema 2 增加声明式 Device List，schema 3 增加插件描述，schema 4 增加原生服务参数，schema 5 增加显式伴随可执行文件。插件 ID 只能包含小写 ASCII 字母、数字和连字符。

```json
{
  "schema": 3,
  "id": "tpms",
  "name": "TPMS",
  "description": "通过蓝牙读取轮胎压力和温度，并显示在 GX Dashboard。",
  "version": "0.1.2",
  "runtime": {
    "kind": "native-service",
    "executable": "bin/venus-tpms-ble"
  },
  "settings": {
    "enabled_path": "/Settings/Plugins/tpms/Enabled"
  },
  "ui": {
    "settings_page": "qml/PageTpmsSettings.qml",
    "dashboard_component": "qml/OverviewTpms.qml",
    "device_list": {
      "value_paths": [
        "com.victronenergy.tpms.main/Slots/front_left/DeviceListValue",
        "com.victronenergy.tpms.main/Slots/front_right/DeviceListValue",
        "com.victronenergy.tpms.main/Slots/rear_left/DeviceListValue",
        "com.victronenergy.tpms.main/Slots/rear_right/DeviceListValue"
      ]
    }
  }
}
```

支持两种 runtime：

- `native-service`：必须提供 manifest 中 `bin/` 下声明的自包含可执行文件。
- `qml-only`：不包含持久后台服务，并且必须声明至少一个 QML 组件。

Schema 4 的 `native-service` 可以声明最多 16 个 `arguments`。Manager 不通过 shell 拼接或展开这些值，而是逐项原样传给可执行文件；服务工作目录固定为该插件的持久配置目录。

Schema 5 可以通过 `companion_executables` 显式声明最多 8 个由主进程调用的同包二进制。Manager 不会单独启动它们，但会像主可执行文件一样验证路径、要求文件真实存在并设置为 `0755`。未声明的普通文件仍保持不可执行。Rathole 使用这一能力将插件适配器与经过校验的上游隧道程序放在同一不可变包中。

Manifest 引用的 Settings 页面和 Dashboard 组件必须位于 `qml/` 下并真实存在。Enabled 路径使用 `/Settings/Plugins/<settings-id>/Enabled`；`settings-id` 等于插件 ID，但其中的连字符必须转换为下划线，以满足 D-Bus 对象路径规则。例如 `loxone-tanks` 使用 `/Settings/Plugins/loxone_tanks/Enabled`。

- `settings_page`：Manager 在插件详情页提供业务设置入口；单独声明它不会向 Device List 增加重复设备行。
- `dashboard_component`：Manager 将已启用插件的组件直接加入 Venus 主 Dashboard 轮播。
- `device_list.value_paths`：可选的 1 至 4 个 Venus D-Bus 数据路径；声明后 Manager 才会为插件生成 Device List 行，并按顺序显示这些值。该字段要求 schema 2 且必须同时声明 `settings_page`。

插件只提供组件，不修改 `PageMain.qml` 或 `main.qml`，也不自行创建菜单入口。关闭插件后，其业务入口会随服务一起隐藏；重新启用时由 Manager 恢复。

插件 QML 从独立的 Package Store 加载。使用 `MbPage`、`OverviewPage` 等 Venus GUI 类型的文件必须显式导入宿主目录：

```qml
import "/opt/victronenergy/gui/qml"
```

Manifest 和 Catalog 按各自 schema 严格解析；未知字段会被拒绝，字段扩展必须通过新的 schema 版本明确演进。Catalog schema 2 提供插件描述，与 manifest schema 独立演进。

## 持久配置

插件包与用户配置严格分离。Manager 根据插件 ID 为每个 `native-service` 创建唯一的持久配置目录：

```text
/data/venus-gx-plugins/config/<plugin-id>/
```

服务启动时通过以下环境变量获得目录，不允许在 manifest 中声明其他配置绝对路径：

```text
VENUS_PLUGIN_ID=<plugin-id>
VENUS_PLUGIN_CONFIG_DIR=/data/venus-gx-plugins/config/<plugin-id>
```

配置目录由 Manager 创建并强制使用 `0700` 权限。Manager 以 `umask 077` 启动插件服务，因此插件新建的配置文件默认只有当前用户可读写。插件只能把用户明确配置且需要跨升级、重装或重启保留的数据写入该目录，并应使用临时文件加原子重命名更新配置。实时遥测、扫描结果、最后接收时间、运行状态和常规日志必须留在内存或 D-Bus，不能周期写入闪存。包内 payload、runit 服务目录和临时下载目录都不能作为持久配置位置。

Manager 不解析配置内容，也不保存配置 schema。配置格式的兼容和升级由插件自身负责。例如 TPMS 的 `config/tpms/state.json` 只保存轮胎绑定，Rathole 使用 `config/rathole/client.toml`。

卸载提供两种明确语义：

- `卸载并保留配置`：停止服务并删除 Registry、payload、QML 引用、服务定义和 Enabled 设置，保留 `config/<plugin-id>/`；重新安装后可继续使用原配置，但默认保持关闭。
- `彻底卸载`：完成同样的卸载流程，并在二次确认后删除整个 `config/<plugin-id>/`。Manager 只删除由合法插件 ID 推导出的真实目录，拒绝符号链接和其他越界路径。

插件升级永远不能删除或覆盖持久配置。

## 安全校验

安装事务在修改当前 Registry 前完成以下校验：

- Catalog 条目的 Ed25519 签名必须能由 Manager 内置的发布公钥验证。
- 包 SHA-256 必须等于 Catalog 提供的预期值。
- 包中 manifest 的 ID 和版本必须等于 Catalog 条目。
- 归档路径必须是 UTF-8 安全相对路径，拒绝绝对路径、`..`、反斜杠和重复路径。
- 拒绝符号链接、硬链接、设备、FIFO 和其他非普通文件类型。
- 拒绝 `manifest.json`、`bin/`、`qml/`、`licenses/` 之外的内容。
- 最多允许 512 个归档条目、8 MiB 压缩包和 16 MiB 解压后普通文件。
- manifest 合同、运行文件和所有被引用 QML 文件必须完整有效。

Catalog 与插件包只允许通过项目固定的 Cloudflare Pages HTTPS 下载边界获取。Catalog 验证成功后只保存在 Manager 内存中；下载失败、格式错误或签名无效时不会产生持久状态。

签名覆盖以下规范化消息：

```text
venus-gx-plugins:v1:<id>:<version>:<sha256>
```

Catalog 使用 `package.signature.key_id` 标识发布密钥，并在 `package.signature.ed25519` 中保存 Base64 编码的签名。签名验证不能替代包 SHA-256 和解包安全校验，三者都会执行。

## Registry 与安装事务

Registry 使用 `schema: 1`，以插件 ID 为键保存完整 manifest、包 SHA-256 和实际 payload 相对路径。启用意图不写入 Registry，只存放在 manifest 声明的 Venus Settings 路径中。

安装和升级采用以下提交顺序：

1. 通过 HTTPS 流式下载到 `/tmp/venus-gx-plugins/`，写入时同时计算并校验 SHA-256。
2. 在 tmpfs 中完成 manifest、包身份、归档结构和解压上限校验；无效包到此终止，不触碰 `/data`。
3. 校验通过后，从 tmpfs 包解压到 `/data` 内的 `staging/`，再原子移动到同一文件系统的 `plugins/<id>/<sha256>/` 不可变目录；压缩包不会复制到闪存。
4. 写入并同步临时 Registry 文件，再原子替换 `registry.json`。
5. 提交成功后删除 tmpfs 包并清理旧 payload；插件配置和 Venus Settings 中的启用意图不属于 payload，因此升级不会覆盖它们。

第 4 步是事务提交点。提交前失败会保留旧 Registry 和旧 payload；卸载则先原子移除 Registry 条目，再清理已失去引用的 payload。

安装事务只处理安装事实，不负责启停插件。事件驱动的 Lifecycle Controller 只在 Manager 启动或用户命令后读取 Venus Settings、runit 服务状态和 QML 可见性，并生成必要的幂等动作。
