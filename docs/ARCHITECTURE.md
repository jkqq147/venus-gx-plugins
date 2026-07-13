# Venus GX Plugins 架构

这是第一版目标架构。Plugin Manager 是 Venus OS 与插件之间唯一的管理边界。

```plantuml
@startuml venus-gx-plugins-architecture

title Venus GX Plugins · 第一版运行时架构

left to right direction
skinparam shadowing false
skinparam roundcorner 6
skinparam componentStyle rectangle
skinparam packageStyle rectangle
skinparam linetype ortho
skinparam defaultFontName Arial

actor "用户" as User

cloud "GitHub Releases" as Releases #e1d5e7 {
    artifact "插件目录\nplugins.json" as RemoteCatalog
    artifact "插件包\n*.vplugin" as RemotePackages
}

node "CCGX\nVenus OS v3.55 · armv7l" as CCGX {
    package "Manager 管理的界面" as ManagerUi #dae8fc {
        component "Settings > Plugins\n管理页面" as ManagementPage
        component "Plugin UI Host\n业务页面与 Dashboard" as UiHost
    }

    package "Plugin Manager" as Manager #d5e8d4 {
        component "D-Bus API" as ManagerApi
        component "Lifecycle\nReconciler" as Reconciler
        component "Catalog 客户端" as CatalogClient
        component "Registry 与\n安装事务" as TransactionEngine
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
User --> UiHost : 使用插件页面
User ..> PluginConfig : 必要时通过 SSH 编辑

ManagementPage <--> ManagerApi : 命令与状态
ManagerApi --> CatalogClient : 用户点击 Refresh
CatalogClient --> RemoteCatalog : HTTPS 获取并校验
CatalogClient --> RemotePackages : 安装时下载
CatalogClient --> CatalogCache : 成功后替换缓存
CatalogCache --> CatalogClient : 断网时读取

ManagerApi --> TransactionEngine : 安装、更新、卸载
TransactionEngine --> LocalRegistry : 原子提交
TransactionEngine --> PackageStore : 校验后写入或删除
TransactionEngine --> Runit : 注册或删除服务定义

ManagerApi --> VenusSettings : 写入用户选择
VenusSettings --> Reconciler : 唯一期望状态
LocalRegistry --> Reconciler : 已安装状态
Reconciler --> Runit : 启动或停止
Reconciler --> UiHost : 显示或隐藏

UiHost --> PluginQml : 动态加载
Runit --> NativePlugin : 运行 native-service
NativePlugin --> PluginConfig : 读取配置
NativePlugin --> BluezDbus : BLE 扫描等设备能力
NativePlugin --> VenusDbus : 发布业务数据
PluginQml --> VenusDbus : 显示业务数据

note bottom of ManagerUi
Plugin Manager 是唯一可以修改
Venus OS 主菜单和插件入口的组件。
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
- Plugin Manager 安装程序只安装管理平台，不捆绑任何插件运行文件。
- 插件不能携带安装脚本、远程 shell hook 或 Python 运行环境。

## 当前实现状态

目前已经完成 Manifest、Catalog、本地 Registry 和安装事务。Registry 中的 `enabled` 字段是 Venus Settings 接入前的临时状态；实现 D-Bus 和 Lifecycle Reconciler 时应移除，避免长期保留两个状态来源。

D-Bus 服务、Lifecycle Reconciler、runit、Venus Settings 和动态 QML Host 尚未接通。
