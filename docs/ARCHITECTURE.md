# Venus GX Plugins 架构

Plugin Manager 是 Venus OS 与插件之间唯一的管理边界。

```plantuml
@startuml venus-gx-plugins-architecture

title Venus GX Plugins · 运行时架构

left to right direction
skinparam shadowing false
skinparam roundcorner 6
skinparam componentStyle rectangle
skinparam packageStyle rectangle
skinparam linetype ortho
skinparam defaultFontName Arial

actor "用户" as User

cloud "GitHub 源站" as GitHub #e1d5e7 {
    artifact "HTTPS 插件目录\nplugins.json" as RemoteCatalog
    artifact "Release 插件包\n*.vplugin" as RemotePackages
    artifact "Manager 发布元数据\nrelease.json" as ManagerRelease
    artifact "Manager 静态二进制" as ManagerBinary
}

cloud "Cloudflare Pages\n静态目录与版本化文件" as CDN #dae8fc

node "CCGX\nVenus OS v3.55 · armv7l" as CCGX {
    package "Manager 管理的界面" as ManagerUi #dae8fc {
        component "Settings > Plugins\n管理页面" as ManagementPage
        component "Device List\n插件直达入口" as DeviceEntries
        component "主 Dashboard\n插件概览组件" as Dashboards
    }

    package "Plugin Manager" as Manager #d5e8d4 {
        component "D-Bus API" as ManagerApi
        component "Lifecycle\nReconciler" as Reconciler
        component "Catalog 客户端\nHTTPS · Ed25519" as CatalogClient
        component "Registry 与\n安装事务" as TransactionEngine
        component "Manager 自更新\n验签与版本检查" as ManagerUpdater
        component "Manager 安装事务\n二进制 · QML · runit" as ManagerInstaller
    }

    package "本地状态" as State #fff2cc {
        database "Venus Settings\nEnabled 期望状态" as VenusSettings
        database "Plugin Registry\n安装元数据" as LocalRegistry
        database "Catalog 缓存\n最后一次有效目录" as CatalogCache
        folder "可变配置\nconfig/<id>/" as PluginConfig
    }

    folder "不可变 Package Store\nplugins/<id>/<sha256>/" as PackageStore #ffe6cc {
        artifact "manifest.json" as InstalledManifest
        artifact "bin/\n原生服务" as NativePlugin
        artifact "qml/\n页面与 Dashboard" as PluginQml
    }

    package "Venus OS 平台能力" as Venus #f5f5f5 {
        component "runit" as Runit
        component "Venus D-Bus" as VenusDbus
        component "BlueZ D-Bus" as BluezDbus
    }
}

User --> ManagementPage : 管理插件
User --> DeviceEntries : 打开业务页面
User --> Dashboards : 查看实时概览
User ..> PluginConfig : 必要时通过 SSH 编辑

ManagementPage <--> ManagerApi : 命令与状态
ManagerApi --> CatalogClient : 用户点击 Refresh
CatalogClient --> CDN : HTTPS 获取
RemoteCatalog --> CDN : 发布时同步 · 短缓存
RemotePackages --> CDN : 发布时同步 · 长缓存
ManagerRelease --> CDN : 发布时同步 · 短缓存
ManagerBinary --> CDN : 发布时同步 · 长缓存
CatalogClient --> CatalogCache : 成功后替换缓存
CatalogCache --> CatalogClient : 断网时读取

ManagerApi --> ManagerUpdater : 用户确认更新
ManagerUpdater --> CDN : 获取元数据与二进制
ManagerUpdater --> ManagerInstaller : 交给新二进制执行

ManagerApi --> TransactionEngine : 安装、更新、卸载
TransactionEngine --> LocalRegistry : 原子提交
TransactionEngine --> PackageStore : 校验后写入或删除
TransactionEngine --> Runit : 注册或删除服务定义

ManagerApi --> VenusSettings : 写入用户选择
VenusSettings --> Reconciler : 唯一期望状态
LocalRegistry --> Reconciler : 已安装状态
Reconciler --> Runit : 启动或停止
Reconciler --> DeviceEntries : 发布或移除入口
Reconciler --> Dashboards : 发布或移除组件

DeviceEntries --> PluginQml : 加载 settings_page
Dashboards --> PluginQml : 加载 dashboard_component
Runit --> NativePlugin : 运行 native-service
NativePlugin --> PluginConfig : 读取配置
NativePlugin --> BluezDbus : BLE 扫描等设备能力
NativePlugin --> VenusDbus : 发布业务数据
PluginQml --> VenusDbus : 显示业务数据

note bottom of ManagerUi
Plugin Manager 是唯一可以修改 Venus OS
主菜单、Device List 和 Dashboard 挂载点的组件。
end note

note bottom of State
插件包不可变，用户配置可变。
Registry 不保存 Enabled 的第二份真相。
end note

@enduml
```

## 架构边界

- Plugin Manager 是唯一可以修改 Venus OS 主菜单和插件入口的组件。
- `/Settings/Plugins/<plugin-id>/Enabled` 是启用状态的唯一真相来源。
- Registry 只记录版本、路径和 SHA-256 等安装元数据。
- 插件包不可变，用户配置独立保存；升级插件不能覆盖配置。
- 关闭插件会停止服务并隐藏业务界面，但保留配置和重新启用入口。
- 普通卸载保留 Manager 管理的 `config/<plugin-id>/`；彻底卸载经二次确认后只删除该插件的配置目录。
- Plugin Manager 安装程序只安装管理平台，不捆绑任何插件运行文件。
- 插件不能携带安装脚本、远程 shell hook 或 Python 运行环境。

## 生命周期契约

生命周期协调只组合三类独立事实：Registry 提供“是否已安装”，Venus Settings 提供“是否应启用”，平台适配器提供服务和界面的实际状态。协调器自身不保存第四份状态。

对 `native-service`，启用时先启动服务再显示界面，关闭时先隐藏界面再停止服务；对 `qml-only`，协调器不会生成服务动作。状态已经一致时不会产生动作，服务启动失败时报告 `Degraded`，由后续协调再次尝试收敛。

## 运行时收敛

Plugin Manager 通过 Venus D-Bus 发布目录、状态、管理命令和已启用插件的 UI 声明。用户操作写入 `/Settings/Plugins/<plugin-id>/Enabled`，协调器每 5 秒结合 Registry 与 runit 实际状态进行幂等收敛。Manager 在 `PageMain.qml` 中只保留一个通用 Device List 模型挂载点，在 `main.qml` 中只保留一个通用 Dashboard 控制器；插件的 Settings 页面和 Dashboard 组件均从不可变 Package Store 动态加载，Device List 最多四个摘要值也只按 manifest 声明的 D-Bus 路径读取，不允许插件自行修改系统 QML。

CCGX 只访问固定的 Cloudflare Pages 下载域名。GitHub Release 发布后，自动化会把目录和版本化文件同步为 Pages 静态资产；设备请求不再实时回源 GitHub。目录只接受 HTTPS，刷新时严格校验 schema、URL、SHA-256 格式和 Ed25519 签名；只有完整有效的目录才会原子替换本地缓存。安装包下载后还会重新校验大小、SHA-256、manifest 和归档内容，Cloudflare 不成为新的信任来源。

Plugin Manager 自身使用静态 ARMv7 二进制安装。Manager 更新元数据与插件 Catalog 分离，更新文件同样经过 Ed25519 和 SHA-256 校验，再交给内置安装事务更新二进制、QML 与服务定义。GUI 重启后必须由主 QML 页面通过 D-Bus 回报就绪；Manager、GUI 进程或该语义握手任一失败，安装事务都会恢复原文件与服务状态。
