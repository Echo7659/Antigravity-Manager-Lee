## 变更类型

- [ ] 修复（bug / 回归）
- [ ] 功能
- [ ] 文档 / 规范 / 发版基建（不夹带进功能 PR；同类文档改动合并为一个 PR）
- [ ] 是否为大功能更改、修复、更新

## 基于的分支与版本号

## 当前问题、对用户使用体验会产生什么问题或影响

## 解決方案

## 提交清单（组内独立、可单独回退）

| 提交 | 作用 | 可否单独 revert |
| -- | -- | ----------- |
|    |    |             |

## 每项提交引起的变化和影响面

## 自检（与 CI 相同的命令）

- [ ] `cd src-tauri && cargo fmt -- --check`
- [ ] `cd src-tauri && cargo clippy --all-targets --all-features`
- [ ] `cd src-tauri && cargo check --locked --all-targets`
- [ ] `bash scripts/check_server_only.sh`
- [ ] `npm run test:dashboard`
- [ ] `npm run build`
- [ ] 定向 Rust 状态、配置与协议兼容性测试通过（不做全量跑测）
- [ ] Linux/amd64 镜像构建、容器 health check 与 `scripts/ci_smoke.py` 通过

## 未验证路径与回滚策略

说明未验证的核心 API、环境限制、旧镜像 digest 与数据挂载保留方式。

## 署名
