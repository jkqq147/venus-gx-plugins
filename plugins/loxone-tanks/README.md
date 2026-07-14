# Loxone Tanks

将 Loxone Miniserver 中的清水、灰水和黑水液位接入 Venus OS，并显示在 GX 原生水箱页面。

## Loxone 准备

在 Loxone 中提供三个只读数值 Sensor，名称分别为：

| Sensor | 水箱 |
| --- | --- |
| `fw tank` | 清水 |
| `gw tank` | 灰水 |
| `bw tank` | 黑水 |

名称忽略大小写和首尾空格，但不做模糊匹配。其他 Sensor、Switch 和执行器不会被接入。

建议为 GX 创建单独的 Loxone 只读账户。

## 配置

1. 在 `Settings > Plugins` 中安装并启用 Loxone Tanks。
2. 打开插件，点击查找 Miniserver，或直接填写其局域网 IP 地址。
3. 输入 Loxone 用户名和密码，然后点击保存并连接。
4. 在 GX 原生水箱页面的 `Setup > Capacity` 中设置水箱容量；容量为 `0` 时只显示百分比。

连接成功后，插件自动验证三个固定 Sensor，无需手动绑定。密码不会保存；设备只保存可撤销的 Loxone 访问令牌。

实时液位只驻留内存和 D-Bus，不写入闪存。容量只在用户确认修改且值确实变化时保存一次。插件不向 Loxone 发送业务控制命令。
