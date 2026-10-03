# Antigravity Manager Lee 发布指南

本指南适用于 Linux 服务与 Web 管理面板。提交、审查与发布治理以根目录 `AGENTS.md` 为准；可执行门禁以 `.github/workflows/ci.yml`、`scripts/release_metadata.py` 和 `scripts/build_release_assets.py` 为准。

## 发布通道

| 通道 | 分支与来源约束 | 版本 / 标签 | 发布行为 |
| --- | --- | --- | --- |
| 正式版 | `main`；commit 必须属于 `origin/main` | `X.Y.Z` 或 `X.Y.Z-lee.N`，标签加 `v` | 正式标签才更新 GHCR latest 与 GitHub Latest Release |
| 预览版 | `beta`；commit 必须属于 `origin/beta` | `X.Y.Z-beta.N` 或 `X.Y.Z-lee.N-beta.N`，标签加 `v` | prerelease=true、makeLatest=false，不更新 latest |

“属于”表示 commit 在对应远程分支的 Git 历史中可达，不要求已发布标签一直停留在不断前进的分支末端。版本号各数字段不得带前导零；Lee 和 beta 序号必须完整。发布门禁只接受表中格式。`-lee.N` 表示 Lee 正式修订，不能仅因含 `-` 就当作 beta。

当前基础版本 `base_version=4.9.1`，完整发布版本 `release_version=4.9.1-lee.1`，标签为 `v4.9.1-lee.1`。manifest、设置页、容器 smoke 与 OCI version 使用完整发布版本；部署包同时记录基础版本和完整发布版本，OCI revision 保留精确 commit。

main/beta 的 push 与 PR 都触发测试；PR 不发布镜像。合法分支构建仅发布 SHA 镜像，不创建 GitHub Release。完整版本标签通过校验后，才发布对应标签镜像与 Release。手动触发工作流也必须满足相同的分支、版本和来源约束。

新功能、重大重构或非平凡修复按维护者暂存规则确认是否先在 beta 实施和验证；已有明确授权时沿用该决定。验证稳定后再按审查流程进入 main。开发分支从 `origin/beta` 或已获授权的 `origin/main` 建立，避免携带本地未审查祖先提交。

## 1. 选择发布提交并同步版本

先检查 remote、tracking、工作树和已有 PR，确认目标分支。工作树包含无关改动时不要直接暂存全部文件；更新目标分支使用 `git fetch` 和 `git pull --ff-only`。不得通过隐式 merge、强推或移动已发布标签解决发布分歧。

`npm run bump` 同步版本文件并生成更新日志骨架：

| 操作 | 命令 | 适用通道 |
| --- | --- | --- |
| 补丁升级；已有 beta 转同号正式版 | `npm run bump patch` | main |
| 次版本 / 主版本升级 | `npm run bump minor` / `npm run bump major` | main |
| 开启下一补丁预览或递增 beta 序号 | `npm run bump beta` | beta |
| 指定正式版本 | `npm run bump X.Y.Z` 或 `npm run bump X.Y.Z-lee.N` | main；将占位符替换为目标版本 |
| 指定预览版本 | `npm run bump X.Y.Z-beta.N` 或 `npm run bump X.Y.Z-lee.N-beta.N` | beta；填写完整数字序号 |

可先追加 `-- --dry-run` 检查拟更新目标。`node scripts/bump-version.mjs --check 4.9.1` 只读核对基础版本及所有服务端版本字段，也可传完整发布版本。脚本会在写入前检查必需文件和字段一致性，不执行编译、推送或发布；严格分支来源约束由发布门禁执行。确认目标版本高于当前已发布版本，禁止复用既有标签。`beta` 递增已有预览序号；`patch` 将 beta 转为同号正式版并保留 Lee 序号。显式指定 Lee beta 可保留当前基础版本。

同步目标为：

- `package.json` 与 `package-lock.json` 的两个根版本；
- `src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`；
- Web 设置页 `src/pages/Settings.tsx` 中的版本显示；
- `CHANGELOG.md`、`CHANGELOG_EN.md`；
- 正式版本的 `README.md`、`README_EN.md`、`README_ZH.md` 标题。

beta 不更新 README 中最新正式版的版本号与摘要。脚本只负责版本同步和日志骨架，不负责写完发布内容或验收；完成后必须检查 diff，不得把脚本成功当作发布成功。

## 2. 补齐变更说明与贡献者归属

选择正确的上次发布标签，完整审计 `<last-tag>..HEAD` 的 Git 历史、作者、`Co-authored-by:` trailers、已合并 PR 及关联 Issue。PR 列表应覆盖整个发布区间，不能只依赖固定数量的最近记录。

两份 changelog 的版本标题必须与将发布的 tag 逐字符一致，包括 `v` 和完整 beta 后缀。例如可使用 `## vX.Y.Z`，或既有列表格式 `**vX.Y.Z-beta.N (日期)**`，其中版本占位符必须替换为实际值。CI 会拒绝缺失或不匹配的标题，不会降级为占位发布文案。

每项变更注明问题、最终行为、影响范围和未验证路径；关联对应的 `Fixes #xxx`、`PR #xxx`。识别出的外部贡献者在对应条目行内使用 `Thanks to @username` 致谢。优先使用贡献者自己的 PR 完成 squash；代理合入时保留作者或明确的 `Co-authored-by:`。不能把工具与文档缺口归责于贡献者。

正式发布还必须人工同步 README 首页的更新摘要：

- `README.md` 与 `README_EN.md` 的 `## 📝 Changelog`；
- `README_ZH.md` 的 `## 📝 更新日志`。

beta 内容只进入两份 changelog，不写入正式版 README 摘要。当前 Release 说明由部署包脚本生成并引用版本历史，不自动从 changelog 抽取所有条目，也未启用自动追加贡献者列表；因此必须确保仓库内的双语归属记录完整。

每个 PR 保持单一问题范围，提交可独立回退。发版基建与无关功能分开审查；提交说明只描述最终实现。完成 PR 模板中的行为变化、未验证路径和回滚策略，并经过同行审查后再合入目标分支。

## 3. 在精确候选提交上执行门禁

版本与说明准备完成后提交并冻结候选 commit，再在该 commit 上执行检查。后续若有任何源文件变化，应重新执行受影响的检查。Rust 使用 1.96、edition 2024，Web 构建使用 Node 20。

```sh
bash scripts/check_server_only.sh
python3 scripts/test_release_metadata.py
node --test scripts/test_bump_version.mjs
node scripts/bump-version.mjs --check
python3 scripts/test_ci_smoke.py
npm ci --legacy-peer-deps
npm run test:dashboard
npm run build
cargo +1.96.0 fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo +1.96.0 check --locked --manifest-path src-tauri/Cargo.toml --all-targets
cargo +1.96.0 clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets --all-features
```

CI 还在镜像发布前执行：

- `server_runtime_`、`admin_model_catalog_`、`model_catalog_`、`opus_5_5_`、`legacy_desktop_fields_are_ignored` 定向测试；
- `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib modules::proxy_db:: -- --test-threads=1`；
- `modules::token_stats::` 与 `proxy::monitor::` 串行定向测试；
- 由 `scripts/prepare_compat_tests.py` 从当前源码生成的独立兼容性 harness，覆盖 `request_compat`、`compat_` 与 `proxy::pipeline`。

状态与数据库测试使用临时数据并串行执行；不对生产数据运行测试。采用定向回归，不要求本地全量测试。若本机缺少容器运行环境或空间不足，记录未执行项，由 Linux CI 完成镜像验证；不能把脚本 fixture 通过写成容器验收通过。

## 4. 推送目标分支并发布标签

只提交已审查的发布文件，推送所选 main 或 beta 分支，并等待该精确 commit 的分支 CI 通过。随后再次 fetch、核对远端分支包含候选 commit，确认版本和 changelog 未变化。

以下命令仅在候选 commit、通道和发布操作均已获授权后执行：

```sh
release_commit="$(git rev-parse HEAD)"
release_version="$(node -p "require('./package.json').version")"
release_tag="v${release_version}"
git tag "$release_tag" "$release_commit" && \
GITHUB_REF_TYPE=tag GITHUB_REF_NAME="$release_tag" \
  GITHUB_SHA="$release_commit" GITHUB_REPOSITORY=Echo7659/Antigravity-Manager-Lee \
  GITHUB_OUTPUT=/dev/null python3 scripts/release_metadata.py && \
git push origin "$release_tag"
```

在推送标签前，必须确认本地校验成功。若失败，停止发布，修正来源、版本或日志问题；不要继续执行后面的 push。CI 会在任何镜像 push 前再次检查：

1. checkout 的 HEAD 等于 GITHUB_SHA，且属于版本对应的 origin/main 或 origin/beta；
2. Web、npm lock、Cargo manifest 与 Cargo lock 版本完全相等；
3. 两份 changelog 存在完整版本标题；
4. tag 精确等于 `v` 加版本，且指向同一 commit。

不得通过修改已发布 tag 绕过门禁。若必须改写共享历史，先检查进行中的 PR、保留本地 `backup/*` 回滚引用，并遵守维护者授权与零内容漂移核对要求。

## 5. CI 镜像与发布包验收

工作流名称为 **Test, Publish Image and Release**，唯一镜像构建入口是 `docker/Dockerfile`。发布目标为 `ghcr.io/echo7659/antigravity-manager-lee` 的 **linux/amd64** 镜像。

构建层使用 Node 20、Rust 1.96 及实际原生依赖；git 用于 boring-sys2 构建时的 init/apply。最终运行层执行 ldd 缺库检查。CI 构建并加载镜像后启动无账号临时容器，验证：

- health 状态与版本；
- 管理接口未鉴权返回 401，管理密码鉴权成功；
- 动态模型目录结构；
- index.html 与 JS/CSS 资源；
- 已删除 API 返回 404；
- 容器自身 health check 最终为 healthy。

全部通过后，CI 给**同一个已验证镜像**打 SHA/版本标签并推送，不重新构建另一个待发布镜像。只有正式标签更新 latest；beta 的 GitHub Release 明确设置 prerelease=true、makeLatest=false。

标签发布生成：

- `antigravity-manager-lee-<完整 tag>-deployment.tar.gz`；
- `image-manifest.json`，记录完整 tag、`base_version`、`release_version`、通道、精确源 commit、平台与镜像 digest；
- `SHA256SUMS`，用于校验部署归档。

归档包含固定镜像 digest 的 compose、空凭据模板、manifest 与部署说明，不包含业务数据或真实密钥。发布验收必须核对 Actions 成功记录、Release 附件、归档校验值以及 `ghcr.io/echo7659/antigravity-manager-lee@sha256:...`；不能只核对可变镜像标签。

## 6. 精确 digest 部署与回滚

生产部署仅使用通过全部 CI 的精确 commit 和 GHCR digest。部署操作需要相应授权，发布成功本身不代表已部署。

部署前记录旧镜像 digest、端口、环境变量、宿主数据路径与账号数量，并保留旧镜像。对原数据目录制作一致性备份；SQLite、账号 JSON、凭据、配置与代理绑定成套保留。容器端口维持 8045，数据挂载目标维持 `/root/.antigravity_tools`。

使用仓库 release compose 时，在 `.env` 中设置已验证的 `IMAGE_DIGEST`；使用发布归档时，compose 已直接固定 digest。两种方式均保持原 `ABV_HOST_DATA_DIR`、密钥和挂载，不用空目录覆盖原数据。具体启动命令见 [Docker 指南](../docker/README.md)。

部署后分别核对容器 health、Web 登录、账号数量、动态模型目录、账号代理绑定、日志与 Token/成本统计，再按发布范围验收真实上游请求。HTTP 200 或 healthy 不能证明真实账号资格、协议转换或计费行为正确。

需要回滚时，恢复旧 IMAGE_DIGEST；固定 image 的发布包则恢复旧镜像引用。以原环境、端口和数据挂载重新创建容器，复核 health 与核心业务。不要删除数据卷或重建账号。只有在明确需要且得到授权时才恢复数据备份，避免覆盖新版本运行期间产生的数据。

## 7. 失败处理

校验失败时先修正具体的分支、版本、标题或 tag 指向错误，再重新提交和验证。不要以忽略失败、强推、复用标签或降低门禁替代修复。

未成功发布的本地标签可在核对精确目标后处理；已推送标签、镜像或 Release 的删除/改写需单独评估引用方并取得相应授权。已发布版本出现问题时优先回滚到已验证的旧 digest，再用新版本号发布修复。记录失败原因、已执行动作和仍未验收项，不承诺固定构建时长。
