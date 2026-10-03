# Lee 镜像与版本发布

当前 Linux/Web-only 发布规则、测试门禁、版本字段、CI 镜像标签和回滚流程统一维护在 [RELEASE_GUIDE.md](RELEASE_GUIDE.md)。本页不再复制一套可能漂移的发布步骤。

必须保持的边界：

- PR 只运行门禁，不发布镜像；`main` / `beta` 分支推送发布以完整 commit SHA 命名的镜像标签，不更新 `latest`。部署以 CI 验证后的镜像摘要为准。
- 只有通过来源、版本、changelog 和完整 CI 校验的正式版本标签才更新 `latest` 并创建 GitHub Release。
- 完整版本同步至 `package.json`、`package-lock.json`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock` 及 Web 版本显示；项目不再包含 `tauri.conf.json`。
- 部署必须设置 `API_KEY`、`WEB_PASSWORD` 和 CI 产出的精确 `IMAGE_DIGEST`，并保留既有数据挂载与旧镜像摘要用于回滚。
- 服务器凭据、账号文件、运行配置和持久化数据不进入 CI，也不提交到 Git。
