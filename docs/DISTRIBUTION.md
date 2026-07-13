# 发布分发

`venus-gx-plugins.pages.dev` 是项目固定的下载边界。Cloudflare Pages 只发布两类静态文件：

- `/catalog/plugins.json` 对应 `master` 分支中的正式目录，缓存 60 秒。
- `/releases/download/<version>/<filename>` 对应同版本的 GitHub Release 文件，成功响应缓存一年。

CCGX 不直接访问 GitHub。Catalog 仍经过 Ed25519 校验，插件包仍经过 SHA-256 与签名校验；Cloudflare 只负责静态分发，不成为新的信任来源。

发布 GitHub Release 后，Actions 会收集所有正式 Release 中的 Manager 和 `.vplugin` 文件，并把完整静态目录部署到 Pages。Manager 的公开文件名统一增加 `.bin` 后缀以适配 Pages 静态路由；版本路径保持不变，历史版本不会被覆盖。

缓存响应头位于 `infra/cloudflare/_headers`，静态目录由 `scripts/build-pages-dist.sh` 生成。自动部署只使用 GitHub Secret `CLOUDFLARE_PAGES_API_TOKEN`，其权限限定为当前账号的 Cloudflare Pages Edit。
