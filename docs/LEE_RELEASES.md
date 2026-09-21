# Lee 镜像与版本发布

## 自动流程

- PR：运行 24 项针对性协议回归测试。
- 推送 main：测试、完整后端镜像构建、容器接口验证，成功后发布 GHCR 的 latest 和 sha-完整提交号标签。
- 推送 v 开头的版本标签：完成相同检查后发布版本镜像，并创建 GitHub Release，附带固定镜像摘要的部署包、清单和 SHA-256 校验文件。
- 发布标签必须与 package.json、Cargo.toml 和 tauri.conf.json 的基础版本一致；允许 -lee.N 后缀标识定制修复，例如 v4.7.12-lee.1。基础应用显示版本仍为 4.7.12，镜像标签与 revision 区分定制发布。

镜像目前针对本项目服务器使用的 linux/amd64。Docker 发布无需额外配置 Docker Hub 密钥，使用 Actions 自动提供的 GITHUB_TOKEN。所有第三方 Action 固定到已核对的提交。

首次创建 GHCR 包后，应确认包对部署服务器可读；公共部署建议将该包的可见性设为 public。仓库的服务器密钥、账号文件和运行配置不进入 Actions，也不提交到 Git。

## 测试边界

scripts/prepare_compat_tests.py 从当前仓库提取实际转换器代码，引用实际回归测试，并隔离不参与测试的签名缓存与音频依赖。隔离测试不替代完整应用构建；发布工作流另外完整构建 Rust 后端，并验证版本、管理鉴权和前端资源。

源码依赖版本取自应用 Cargo.lock，测试依赖另有 tests/compat/Cargo.lock 固定。升级依赖时需要重新生成并验证该锁文件。

## 部署

新部署可使用 docker/docker-compose.release.yml；先设置 API_KEY 和 WEB_PASSWORD，并保留 data 目录。已有服务器应只替换镜像，继续使用原来的端口、密钥及账号数据挂载。

上游已经撤下的模型可能返回提示文本但不携带用量。此类结果不应伪造 token 数字；应依据实际可用模型调整运行配置中的 custom_mapping。当前已验证 gemini-3.7-flash-medium 和 gemini-3.8-flash-medium 可正常生成并返回用量。

桌面签名打包与 GitHub Pages 不在自动 Docker 发布流程中；Pages 工作流保留为手动触发。

镜像构建同时运行完整工程中的 `compat_` 和日志数据库测试，数据库测试串行执行以隔离全局测试目录。前端从当前源码构建，与后端基础版本一致。
